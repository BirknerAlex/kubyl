//! The XRay tab: the tree as a table, one line per object.

use std::cell::RefCell;
use std::collections::{HashMap, HashSet};
use std::rc::Rc;
use std::sync::Arc;
use std::time::Duration;

use gpui::{
    App, AppContext as _, Context, Entity, FocusHandle, Focusable, FontWeight, IntoElement,
    KeyBinding, Render, SharedString, Subscription, Task, Window, actions, div, prelude::*,
};
use gpui_component::input::{Input, InputEvent, InputState};
use kubyl_core::actions::OpenView;
use kubyl_core::{
    ActionRegistry, ActionSpec, ActiveContext, CellValue, ClusterId, ColumnDef, ColumnWidth, Gvr,
    ResourceRef, TabView, Tone, ViewKind, ViewRegistry, ViewRequest,
};
use kubyl_kube::ConnectionManager;
use kubyl_resources::store::find_resource;
use kubyl_resources::{ResourceStores, StoreHandle, StoreKey};
use kubyl_ui::{
    ActiveColors, Chip, DataTable, DataTableEvent, Icon, IconName, TableDelegate, TreeNode, fonts,
    h_flex, sizes, u, v_flex,
};
use kubyl_xray_core::{Filter, Health, Inputs, Node, Root, Row, build, flatten};
use serde_json::Value;

actions!(
    xray_view,
    [
        /// Opens the branch under the cursor.
        Expand,
        /// Closes the branch under the cursor, or goes to its parent.
        Collapse,
        /// Opens every branch.
        ExpandAll,
        /// Closes every branch.
        CollapseAll,
        /// Opens the page of the object under the cursor.
        OpenObject,
        /// Opens or closes the branch under the cursor.
        ToggleRow,
        /// Types a filter.
        FocusFilter,
        /// Clears the filter and goes back to the tree.
        ClearFilter,
        /// Goes from the filter to the tree, keeping the filter.
        FocusTree,
    ]
);

/// Key context of the tab.
pub const CONTEXT: &str = "XrayView";
const FILTER_CONTEXT: &str = "XrayFilter";

pub(crate) fn init(cx: &mut App) {
    cx.bind_keys([
        KeyBinding::new("right", Expand, Some(CONTEXT)),
        KeyBinding::new("left", Collapse, Some(CONTEXT)),
        KeyBinding::new("shift-right", ExpandAll, Some(CONTEXT)),
        KeyBinding::new("shift-left", CollapseAll, Some(CONTEXT)),
        KeyBinding::new("o", OpenObject, Some(CONTEXT)),
        KeyBinding::new("space", ToggleRow, Some(CONTEXT)),
        KeyBinding::new("/", FocusFilter, Some(CONTEXT)),
        KeyBinding::new("escape", ClearFilter, Some(CONTEXT)),
        KeyBinding::new("escape", ClearFilter, Some(FILTER_CONTEXT)),
        KeyBinding::new("down", FocusTree, Some(FILTER_CONTEXT)),
    ]);
    ActionRegistry::register(
        cx,
        ActionSpec::new("XRay: Expand", Expand)
            .hint("Expand")
            .in_context(CONTEXT),
    );
    ActionRegistry::register(
        cx,
        ActionSpec::new("XRay: Collapse", Collapse)
            .hint("Collapse")
            .in_context(CONTEXT),
    );
    ActionRegistry::register(
        cx,
        ActionSpec::new("XRay: Expand All", ExpandAll).in_context(CONTEXT),
    );
    ActionRegistry::register(
        cx,
        ActionSpec::new("XRay: Collapse All", CollapseAll).in_context(CONTEXT),
    );
    ActionRegistry::register(
        cx,
        ActionSpec::new("XRay: Open Object", OpenObject)
            .hint("Open")
            .in_context(CONTEXT),
    );
}

/// What the tree is built from.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
enum Slot {
    Pods,
    Services,
    Deployments,
    ReplicaSets,
    StatefulSets,
    DaemonSets,
    ConfigMaps,
    Secrets,
    Claims,
    ServiceAccounts,
}

impl Slot {
    /// Group, plural, and whether metadata is enough.
    fn resource(self) -> (&'static str, &'static str, bool) {
        match self {
            Slot::Pods => ("", "pods", false),
            Slot::Services => ("", "services", false),
            Slot::Deployments => ("apps", "deployments", false),
            Slot::ReplicaSets => ("apps", "replicasets", false),
            Slot::StatefulSets => ("apps", "statefulsets", false),
            Slot::DaemonSets => ("apps", "daemonsets", false),
            // Only names matter; Secret data is never fetched.
            Slot::ConfigMaps => ("", "configmaps", true),
            Slot::Secrets => ("", "secrets", true),
            Slot::Claims => ("", "persistentvolumeclaims", true),
            // Full: their Secret names are children (names only, never Secret data).
            Slot::ServiceAccounts => ("", "serviceaccounts", false),
        }
    }

    fn for_root(root: Root) -> Vec<Slot> {
        let mut slots = vec![
            Slot::Pods,
            Slot::ConfigMaps,
            Slot::Secrets,
            Slot::Claims,
            Slot::ServiceAccounts,
        ];
        match root {
            Root::Pods => {}
            Root::Services => slots.push(Slot::Services),
            Root::Deployments => slots.push(Slot::Deployments),
            Root::ReplicaSets => slots.push(Slot::ReplicaSets),
            Root::StatefulSets => slots.push(Slot::StatefulSets),
            Root::DaemonSets => slots.push(Slot::DaemonSets),
        }
        slots
    }
}

struct Delegate {
    rows: Rc<RefCell<Vec<Row>>>,
}

fn tone(health: Health) -> Tone {
    match health {
        Health::Ok => Tone::Good,
        Health::Warning => Tone::Warning,
        Health::Bad => Tone::Bad,
        Health::Neutral => Tone::Muted,
    }
}

impl TableDelegate for Delegate {
    fn columns(&self) -> Vec<ColumnDef> {
        vec![
            ColumnDef::new(
                "resource",
                "Resource",
                ColumnWidth::Flex {
                    weight: 3.0,
                    min: 280.0,
                },
            )
            .mono(),
            ColumnDef::new("kind", "Kind", ColumnWidth::Fixed(150.0)),
            ColumnDef::new(
                "status",
                "Status",
                ColumnWidth::Flex {
                    weight: 1.0,
                    min: 160.0,
                },
            ),
        ]
    }

    fn row_count(&self, _: &App) -> usize {
        self.rows.borrow().len()
    }

    fn tree_node(&self, row: usize, column: usize, _: &App) -> Option<TreeNode> {
        let rows = self.rows.borrow();
        let r = rows.get(row).filter(|_| column == 0)?;
        Some(TreeNode {
            depth: r.depth,
            expanded: r.expandable.then_some(r.expanded),
        })
    }

    fn cell(&self, row: usize, column: usize, _: &App) -> CellValue {
        let rows = self.rows.borrow();
        let Some(r) = rows.get(row) else {
            return CellValue::Empty;
        };
        match column {
            0 => CellValue::Text(r.node.name.clone().into()),
            1 => CellValue::Text(r.node.kind.into()),
            _ => {
                // A collapsed branch still shows the trouble below it.
                let health = if r.expanded { r.node.health } else { r.health };
                if r.node.info.is_empty() && health == Health::Neutral {
                    CellValue::Empty
                } else {
                    CellValue::Status {
                        label: r.node.info.clone().into(),
                        tone: tone(health),
                    }
                }
            }
        }
    }
}

pub struct XrayView {
    cluster: ClusterId,
    root: Root,
    namespace: Option<String>,
    /// An object to open the tree on (from a row's menu), until it is found.
    focus_on: Option<String>,
    stores: HashMap<Slot, StoreHandle>,
    observers: Vec<Subscription>,
    tree: Vec<Node>,
    expanded: HashSet<String>,
    /// Namespace nodes already opened by default.
    seen: HashSet<String>,
    rows: Rc<RefCell<Vec<Row>>>,
    table: Entity<DataTable>,
    filter_input: Entity<InputState>,
    filter: Filter,
    filter_error: Option<String>,
    focus: FocusHandle,
    pending: Option<Task<()>>,
    _subscriptions: Vec<Subscription>,
}

impl XrayView {
    pub fn new(target: ResourceRef, window: &mut Window, cx: &mut Context<Self>) -> Self {
        let rows = Rc::new(RefCell::new(Vec::new()));
        let table = cx.new(|cx| DataTable::new(Delegate { rows: rows.clone() }, cx));
        let mut subscriptions =
            vec![
                cx.subscribe(&table, |this, _, event: &DataTableEvent, cx| match event {
                    DataTableEvent::Confirmed(ix) => this.confirm(*ix, cx),
                    DataTableEvent::Toggled(_) => this.toggle_selected(cx),
                    DataTableEvent::SelectionChanged(_) => {}
                }),
            ];
        let filter_input = cx.new(|cx| {
            InputState::new(window, cx).placeholder("Filter: regex, !regex, -f fuzzy, -l labels")
        });
        subscriptions.push(cx.subscribe_in(
            &filter_input,
            window,
            |this, input, event: &InputEvent, window, cx| match event {
                InputEvent::Change => {
                    let text = input.read(cx).value().to_string();
                    this.set_filter(&text, cx);
                }
                InputEvent::PressEnter { .. } => this.focus_tree(window, cx),
                _ => {}
            },
        ));
        // Discovery arrives after the connection: watch what's served as soon as it is.
        if let Some(manager) = ConnectionManager::try_global(cx) {
            subscriptions.push(cx.observe(&manager, |this, _, cx| this.sync(cx)));
        }
        let mut this = Self {
            cluster: target.cluster,
            root: crate::root_of(&target.gvr).unwrap_or(Root::Deployments),
            namespace: target.namespace,
            focus_on: target.name,
            stores: HashMap::new(),
            observers: Vec::new(),
            tree: Vec::new(),
            expanded: HashSet::new(),
            seen: HashSet::new(),
            rows,
            table,
            filter_input,
            filter: Filter::default(),
            filter_error: None,
            focus: cx.focus_handle(),
            pending: None,
            _subscriptions: subscriptions,
        };
        this.sync(cx);
        this
    }

    fn resolve(&self, group: &str, resource: &str, cx: &App) -> Option<Gvr> {
        let discovery = ConnectionManager::try_global(cx)?
            .read(cx)
            .discovery(&self.cluster)?;
        find_resource(&discovery.resources, &Gvr::new(group, "", resource))
            .map(|info| info.gvr.clone())
    }

    /// Watches what the root needs; only changes start or stop watches.
    fn sync(&mut self, cx: &mut Context<Self>) {
        let mut changed = false;
        let wanted = Slot::for_root(self.root);
        let before = self.stores.len();
        self.stores.retain(|slot, _| wanted.contains(slot));
        changed |= self.stores.len() != before;
        for slot in wanted {
            if self.stores.contains_key(&slot) {
                continue;
            }
            let (group, resource, metadata) = slot.resource();
            let Some(gvr) = self.resolve(group, resource, cx) else {
                continue;
            };
            let mut key = StoreKey::new(self.cluster.clone(), gvr, None);
            if metadata {
                key = key.metadata();
            }
            self.stores.insert(slot, ResourceStores::acquire(cx, key));
            changed = true;
        }
        if changed {
            self.observers = self
                .stores
                .values()
                .map(|s| cx.observe(s.entity(), |this, _, cx| this.schedule(cx)))
                .collect();
            self.schedule(cx);
        }
    }

    /// Rebuilds the tree shortly after the stores change (a busy cluster changes them
    /// constantly).
    fn schedule(&mut self, cx: &mut Context<Self>) {
        if self.pending.is_some() {
            return;
        }
        self.pending = Some(cx.spawn(async move |this, cx| {
            cx.background_executor()
                .timer(Duration::from_millis(250))
                .await;
            this.update(cx, |this, cx| {
                this.pending = None;
                this.rebuild(cx);
            })
            .ok();
        }));
    }

    fn objects(&self, slot: Slot, cx: &App) -> Vec<Arc<Value>> {
        let Some(store) = self.stores.get(&slot) else {
            return Vec::new();
        };
        store
            .read(cx)
            .objects()
            .values()
            .filter(|o| {
                self.namespace.is_none()
                    || o.pointer("/metadata/namespace").and_then(Value::as_str)
                        == self.namespace.as_deref()
            })
            .cloned()
            .collect()
    }

    fn rebuild(&mut self, cx: &mut Context<Self>) {
        let inputs = Inputs {
            pods: self.objects(Slot::Pods, cx),
            services: self.objects(Slot::Services, cx),
            deployments: self.objects(Slot::Deployments, cx),
            replica_sets: self.objects(Slot::ReplicaSets, cx),
            stateful_sets: self.objects(Slot::StatefulSets, cx),
            daemon_sets: self.objects(Slot::DaemonSets, cx),
            config_maps: self.objects(Slot::ConfigMaps, cx),
            secrets: self.objects(Slot::Secrets, cx),
            claims: self.objects(Slot::Claims, cx),
            service_accounts: self.objects(Slot::ServiceAccounts, cx),
        };
        self.tree = build(self.root, &inputs, self.filter.labels());
        // Namespaces start open, once; the user may close them.
        for ns in &self.tree {
            if self.seen.insert(ns.id()) {
                self.expanded.insert(ns.id());
            }
        }
        let mut select = None;
        if let Some(name) = &self.focus_on {
            let found = self.tree.iter().find_map(|ns| {
                let node = ns.children.iter().find(|n| &n.name == name)?;
                Some(format!("{}>{}", ns.id(), node.id()))
            });
            if let Some(path) = found {
                self.expanded.insert(path.clone());
                select = Some(path);
                self.focus_on = None;
            }
        }
        self.refresh(cx);
        if let Some(path) = select {
            let ix = self.rows.borrow().iter().position(|r| r.path == path);
            self.table.update(cx, |table, cx| table.select(ix, cx));
        }
    }

    /// Flattens the tree into the table, keeping the cursor on the same object.
    fn refresh(&mut self, cx: &mut Context<Self>) {
        let selected = self.selected_path(cx);
        *self.rows.borrow_mut() = flatten(&self.tree, &self.expanded, &self.filter);
        let ix = selected
            .and_then(|path| self.rows.borrow().iter().position(|r| r.path == path))
            .or_else(|| (!self.rows.borrow().is_empty()).then_some(0));
        self.table.update(cx, |table, cx| {
            table.select(ix, cx);
            cx.notify();
        });
        cx.notify();
    }

    fn selected_row(&self, cx: &App) -> Option<Row> {
        let ix = self.table.read(cx).selected()?;
        self.rows.borrow().get(ix).cloned()
    }

    fn selected_path(&self, cx: &App) -> Option<String> {
        self.selected_row(cx).map(|r| r.path)
    }

    fn expand(&mut self, _: &Expand, _: &mut Window, cx: &mut Context<Self>) {
        if let Some(row) = self.selected_row(cx).filter(|r| r.expandable) {
            self.expanded.insert(row.path);
            self.refresh(cx);
        }
    }

    fn collapse(&mut self, _: &Collapse, _: &mut Window, cx: &mut Context<Self>) {
        let Some(row) = self.selected_row(cx) else {
            return;
        };
        if row.expanded {
            self.expanded.remove(&row.path);
            self.refresh(cx);
        } else if let Some((parent, _)) = row.path.rsplit_once('>') {
            // Already closed: go up.
            let ix = self.rows.borrow().iter().position(|r| r.path == parent);
            self.table.update(cx, |table, cx| table.select(ix, cx));
        }
    }

    fn expand_all(&mut self, _: &ExpandAll, _: &mut Window, cx: &mut Context<Self>) {
        fn walk(nodes: &[Node], parent: &str, out: &mut HashSet<String>) {
            for node in nodes {
                let path = if parent.is_empty() {
                    node.id()
                } else {
                    format!("{parent}>{}", node.id())
                };
                walk(&node.children, &path, out);
                out.insert(path);
            }
        }
        walk(&self.tree, "", &mut self.expanded);
        self.refresh(cx);
    }

    fn collapse_all(&mut self, _: &CollapseAll, _: &mut Window, cx: &mut Context<Self>) {
        self.expanded.clear();
        self.refresh(cx);
    }

    /// Enter: opens the page of the object (namespaces and containers have none).
    fn confirm(&mut self, ix: usize, cx: &mut Context<Self>) {
        let Some(row) = self.rows.borrow().get(ix).cloned() else {
            return;
        };
        self.open_row(&row, cx);
    }

    fn toggle_row(&mut self, _: &ToggleRow, _: &mut Window, cx: &mut Context<Self>) {
        self.toggle_selected(cx);
    }

    fn toggle_selected(&mut self, cx: &mut Context<Self>) {
        if let Some(row) = self.selected_row(cx).filter(|r| r.expandable)
            && !self.expanded.remove(&row.path)
        {
            self.expanded.insert(row.path);
        }
        self.refresh(cx);
    }

    /// Applies what was typed. A label selector changes what the tree starts from, so it
    /// rebuilds; a text filter only prunes.
    fn set_filter(&mut self, text: &str, cx: &mut Context<Self>) {
        match Filter::parse(text) {
            Ok(filter) => {
                let rebuild = filter.labels() != self.filter.labels();
                self.filter = filter;
                self.filter_error = None;
                if rebuild {
                    self.schedule(cx);
                } else {
                    self.refresh(cx);
                }
            }
            Err(err) => {
                self.filter_error = Some(err);
                cx.notify();
            }
        }
    }

    fn focus_filter(&mut self, _: &FocusFilter, window: &mut Window, cx: &mut Context<Self>) {
        let focus = self.filter_input.read(cx).focus_handle(cx);
        focus.focus(window, cx);
    }

    fn focus_tree(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.focus_handle(cx).focus(window, cx);
    }

    fn focus_tree_action(&mut self, _: &FocusTree, window: &mut Window, cx: &mut Context<Self>) {
        self.focus_tree(window, cx);
    }

    fn clear_filter(&mut self, _: &ClearFilter, window: &mut Window, cx: &mut Context<Self>) {
        self.filter_input
            .update(cx, |input, cx| input.set_value("", window, cx));
        self.set_filter("", cx);
        self.focus_tree(window, cx);
    }

    fn open_object(&mut self, _: &OpenObject, _: &mut Window, cx: &mut Context<Self>) {
        if let Some(row) = self.selected_row(cx) {
            self.open_row(&row, cx);
        }
    }

    fn open_row(&mut self, row: &Row, cx: &mut Context<Self>) {
        let node = &row.node;
        if node.resource.is_empty() {
            return;
        }
        let Some(gvr) = self.resolve(node.group, node.resource, cx) else {
            return;
        };
        let kind = ViewRegistry::object_view(cx, &gvr);
        let target = ResourceRef::object(
            self.cluster.clone(),
            gvr,
            node.namespace.clone(),
            node.name.clone(),
        );
        let request = ViewRequest::for_resource(kind, target);
        cx.defer(move |cx| {
            if let Some(window) = cx.active_window().or_else(|| cx.windows().first().copied()) {
                window
                    .update(cx, |_, window, cx| {
                        window.dispatch_action(Box::new(OpenView(request)), cx)
                    })
                    .ok();
            }
        });
    }

    fn set_root(&mut self, root: Root, cx: &mut Context<Self>) {
        if self.root != root {
            self.root = root;
            self.expanded.clear();
            self.seen.clear();
            self.tree.clear();
            self.sync(cx);
            self.rebuild(cx);
        }
    }

    fn loading(&self, cx: &App) -> bool {
        self.stores.is_empty()
            || self
                .stores
                .values()
                .any(|s| !s.read(cx).status().is_settled())
    }

    fn render_toolbar(&self, cx: &mut Context<Self>) -> impl IntoElement + use<> {
        let colors = cx.colors().clone();
        h_flex()
            .flex_none()
            .h(u(sizes::TOOLBAR))
            .px(u(12.0))
            .gap(u(8.0))
            .border_b_1()
            .border_color(colors.border_variant)
            .child(
                h_flex()
                    .gap(u(6.0))
                    .text_color(colors.text_dim)
                    .child(Icon::new(IconName::Network).color(colors.accent))
                    .child(
                        div()
                            .text_color(colors.text)
                            .font_weight(FontWeight::MEDIUM)
                            .child("XRay"),
                    )
                    .child("·")
                    .child(
                        self.namespace
                            .clone()
                            .unwrap_or_else(|| "all namespaces".into()),
                    )
                    .child(format!(
                        "· {} {}",
                        self.tree.iter().map(|ns| ns.children.len()).sum::<usize>(),
                        self.root.title().to_lowercase()
                    )),
            )
            .child(
                div()
                    .key_context(FILTER_CONTEXT)
                    .on_action(cx.listener(Self::clear_filter))
                    .on_action(cx.listener(Self::focus_tree_action))
                    .flex_none()
                    .w(u(300.0))
                    .h(u(sizes::CONTROL))
                    .px(u(8.0))
                    .flex()
                    .items_center()
                    .gap(u(7.0))
                    .rounded(u(5.0))
                    .bg(colors.input_background)
                    .border_1()
                    .border_color(if self.filter_error.is_some() {
                        colors.red
                    } else {
                        colors.border
                    })
                    .font_family(fonts::MONO)
                    .text_size(u(12.0))
                    .child(Icon::new(IconName::Funnel).size(12.0))
                    .child(
                        div()
                            .flex_1()
                            .min_w_0()
                            .child(Input::new(&self.filter_input).appearance(false)),
                    ),
            )
            .child(div().flex_1())
            .children(Root::ALL.map(|root| {
                div()
                    .id(SharedString::from(format!("xray-root-{}", root.id())))
                    .cursor_pointer()
                    .on_click(cx.listener(move |this, _, _, cx| this.set_root(root, cx)))
                    .child(Chip::new(root.id()).selected(self.root == root))
            }))
    }
}

impl Focusable for XrayView {
    fn focus_handle(&self, cx: &App) -> FocusHandle {
        if self.rows.borrow().is_empty() {
            self.focus.clone()
        } else {
            self.table.read(cx).focus_handle(cx)
        }
    }
}

impl TabView for XrayView {
    fn tab_title(&self, _: &App) -> SharedString {
        format!("XRay {}", self.root.title()).into()
    }

    fn tab_icon(&self, _: &App) -> Option<SharedString> {
        Some(IconName::Network.path())
    }

    fn tab_dot(&self, cx: &App) -> Option<gpui::Hsla> {
        let active = ActiveContext::global(cx)
            .cluster
            .as_ref()
            .map(|c| c.id.clone());
        (active.as_ref() != Some(&self.cluster))
            .then(|| ConnectionManager::try_global(cx).map(|m| m.read(cx).color(&self.cluster, cx)))
            .flatten()
    }

    fn view_request(&self, _: &App) -> Option<ViewRequest> {
        Some(ViewRequest::for_resource(
            ViewKind::Custom(crate::VIEW_KIND.into()),
            crate::target(&self.cluster, self.root, self.namespace.clone()),
        ))
    }
}

impl Render for XrayView {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let colors = cx.colors().clone();
        let toolbar = self.render_toolbar(cx);
        let empty = self.rows.borrow().is_empty();
        let body = if empty {
            let text = if self.loading(cx) {
                "Loading…".to_string()
            } else {
                format!("No {} to show.", self.root.title().to_lowercase())
            };
            v_flex()
                .flex_1()
                .items_center()
                .justify_center()
                .text_color(colors.text_dim)
                .child(text)
                .into_any_element()
        } else {
            div()
                .flex_1()
                .min_h_0()
                .flex()
                .child(self.table.clone())
                .into_any_element()
        };
        v_flex()
            .id("xray")
            .key_context(CONTEXT)
            .track_focus(&self.focus)
            .on_action(cx.listener(Self::expand))
            .on_action(cx.listener(Self::collapse))
            .on_action(cx.listener(Self::expand_all))
            .on_action(cx.listener(Self::collapse_all))
            .on_action(cx.listener(Self::open_object))
            .on_action(cx.listener(Self::toggle_row))
            .on_action(cx.listener(Self::focus_filter))
            .on_action(cx.listener(Self::clear_filter))
            .size_full()
            .bg(colors.background)
            .child(toolbar)
            .child(body)
    }
}
