//! A Flux object's tab (board 21): Summary (status, conditions, source and revision with a
//! commit link, the kind's settings, dependencies; for HelmReleases the chart, values sources,
//! remediation, failure counters and the link to the Helm release; for sources the artifact and
//! what uses it), Inventory (what it applied, resolved against Kubyl's live caches, with
//! children), History, Events and the controller's logs.

use std::collections::{BTreeSet, HashMap};
use std::sync::Arc;

use gpui::{
    AnyElement, App, Context, FocusHandle, Focusable, FontWeight, IntoElement, Render,
    SharedString, Subscription, Window, div, prelude::*,
};
use gpui_component::button::{Button as MenuButton, ButtonVariants as _};
use gpui_component::menu::{DropdownMenu as _, PopupMenuItem};
use kubyl_core::actions::{AskAgent, OpenView};
use kubyl_core::{
    ClusterId, ColumnDef, ColumnWidth, Gvr, ResourceRef, TabView, Tone, ViewKind, ViewRegistry,
    ViewRequest,
};
use kubyl_flux_core::agent::{self, Unhealthy, Warning};
use kubyl_flux_core::deps::{Dependency, Graph};
use kubyl_flux_core::details::{
    self, DataRef, HelmReleaseSpec, KustomizationSpec, failures, helm_history, helm_storage,
    kustomization_history,
};
use kubyl_flux_core::inventory::{self, Entry, NodeHealth};
use kubyl_flux_core::kinds::FluxKind;
use kubyl_flux_core::links;
use kubyl_flux_core::model::{FluxObject, ObjectRef, State, commit_of, short_revision};
use kubyl_flux_core::ops::Action;
use kubyl_kube::ConnectionManager;
use kubyl_resources::{
    ResourceSelection, ResourceStores, Selected, StoreHandle, StoreKey, StoreStatus,
};
use kubyl_ui::{
    ActiveColors, Button, Chip, Colors, Icon, IconName, KeyHints, SelectionScope, fonts, h_flex, u,
    v_flex,
};
use serde_json::Value;

use crate::actions::{self, OBJECT_CONTEXT};
use crate::state::{self, Flux};
use crate::widgets;

/// `ViewKind::Custom` of an object's tab.
pub const VIEW_KIND: &str = "flux_object";
/// Inventory entries resolved at once; more on request.
const INVENTORY_PAGE: usize = 200;
/// Above this many namespaces of one kind, the inventory watches the kind in all namespaces
/// instead of one watch per namespace.
const INVENTORY_NAMESPACES: usize = 8;
/// "Ask agent" waits at most this long for the Events and the inventory to load.
const AGENT_WAIT: std::time::Duration = std::time::Duration::from_secs(5);

/// Kinds whose health the inventory computes (`inventory::health`), with their children: the
/// whole object is watched. Everything else is resolved from metadata only (presence, owner
/// references, labels and annotations), which never pulls Secret data into memory.
fn needs_full_object(group: &str, kind: &str) -> bool {
    matches!(
        (group, kind),
        (
            "apps",
            "Deployment" | "StatefulSet" | "DaemonSet" | "ReplicaSet"
        ) | ("batch", "Job" | "CronJob")
            | ("", "Pod" | "PersistentVolumeClaim" | "Service")
    ) || kubyl_flux_core::kinds::is_flux_group(group)
}

/// The sub-tabs.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash)]
pub enum Tab {
    #[default]
    Summary,
    Inventory,
    History,
    Events,
    Logs,
}

impl Tab {
    pub fn label(self) -> &'static str {
        match self {
            Tab::Summary => "Summary",
            Tab::Inventory => "Inventory",
            Tab::History => "History",
            Tab::Events => "Events",
            Tab::Logs => "Controller logs",
        }
    }

    fn of(kind: FluxKind) -> Vec<Tab> {
        match kind {
            FluxKind::Kustomization | FluxKind::HelmRelease => vec![
                Tab::Summary,
                Tab::Inventory,
                Tab::History,
                Tab::Events,
                Tab::Logs,
            ],
            _ => vec![Tab::Summary, Tab::Events, Tab::Logs],
        }
    }
}

/// A ReplicaSet, Job or Pod under its owner in the inventory tree.
struct Child {
    group: &'static str,
    kind: &'static str,
    gvr: Gvr,
    object: Arc<Value>,
}

/// A row of the inventory tree.
#[derive(Clone, Debug)]
struct Node {
    depth: usize,
    kind: String,
    name: String,
    namespace: Option<String>,
    gvr: Option<Gvr>,
    health: Option<NodeHealth>,
    has_children: bool,
    key: String,
}

pub struct ObjectView {
    target: ResourceRef,
    kind: FluxKind,
    tab: Tab,
    own: StoreHandle,
    object: Option<FluxObject>,
    gone: bool,
    /// Same-kind objects (dependencies) and the sources (links, commit URLs).
    related: Vec<(FluxKind, StoreHandle)>,
    events: Option<StoreHandle>,
    /// Stores the inventory is resolved against, by (gvr, namespace).
    inventory_stores: HashMap<(Gvr, Option<String>), StoreHandle>,
    inventory_shown: usize,
    collapsed: BTreeSet<String>,
    /// The object's inventory, parsed when the object changes.
    inventory: Option<Arc<Vec<Entry>>>,
    /// Related objects, re-parsed only when their store changed.
    parsed: state::ParsedObjects,
    focus: FocusHandle,
    /// The tab has focus: only then does it publish its object as the selection.
    focused: bool,
    /// "Ask agent" waiting for the Events and the inventory.
    _ask: Option<gpui::Task<()>>,
    _observers: Vec<Subscription>,
    _inventory_observers: Vec<Subscription>,
    _subscriptions: Vec<Subscription>,
}

impl ObjectView {
    pub fn new(mut target: ResourceRef, window: &mut Window, cx: &mut Context<Self>) -> Self {
        let kind = actions::kind_of(&target).unwrap_or(FluxKind::Kustomization);
        // Links name the GA version; watch the one the cluster serves.
        if let Some(gvr) = state::gvr(&target.cluster, kind, cx) {
            target.gvr = gvr;
        }
        let own = ResourceStores::acquire(
            cx,
            StoreKey::new(
                target.cluster.clone(),
                target.gvr.clone(),
                target.namespace.clone(),
            )
            .fields(format!(
                "metadata.name={}",
                target.name.as_deref().unwrap_or_default()
            )),
        );
        let mut subscriptions = vec![cx.observe(own.entity(), |this, _, cx| this.refresh(cx))];
        if let Some(flux) = Flux::try_global(cx) {
            subscriptions.push(cx.observe(&flux, |_, _, cx| cx.notify()));
        }
        if let Some(manager) = ConnectionManager::try_global(cx) {
            let cluster = target.cluster.clone();
            subscriptions.push(cx.subscribe(
                &manager,
                move |this, _, event: &kubyl_kube::ConnectionEvent, cx| {
                    if matches!(event, kubyl_kube::ConnectionEvent::DiscoveryChanged(id) if *id == cluster)
                    {
                        this.watch_related(cx);
                        this.refresh(cx);
                    }
                },
            ));
        }
        let tab = Tab::default();
        let focus = cx.focus_handle();
        subscriptions.push(cx.on_focus_in(&focus, window, |this: &mut Self, _, cx| {
            this.focused = true;
            this.publish_selection(cx);
        }));
        subscriptions.push(cx.on_focus_out(&focus, window, |this: &mut Self, _, _, _| {
            this.focused = false;
        }));
        let mut this = Self {
            kind,
            tab,
            own,
            object: None,
            gone: false,
            related: Vec::new(),
            events: None,
            inventory_stores: HashMap::new(),
            inventory_shown: INVENTORY_PAGE,
            collapsed: BTreeSet::new(),
            inventory: None,
            parsed: state::ParsedObjects::default(),
            focus,
            focused: false,
            _ask: None,
            _observers: Vec::new(),
            _inventory_observers: Vec::new(),
            _subscriptions: subscriptions,
            target,
        };
        this.watch_related(cx);
        this.refresh(cx);
        this
    }

    fn cluster(&self) -> &ClusterId {
        &self.target.cluster
    }

    fn name(&self) -> &str {
        self.target.name.as_deref().unwrap_or_default()
    }

    fn namespace(&self) -> &str {
        self.target.namespace.as_deref().unwrap_or_default()
    }

    /// Watches the kinds the summary links to: same-kind objects (dependencies, "used by"),
    /// sources, and for sources the objects that may use them.
    fn watch_related(&mut self, cx: &mut Context<Self>) {
        let mut kinds: Vec<FluxKind> = vec![self.kind];
        if matches!(self.kind, FluxKind::Kustomization | FluxKind::HelmRelease) {
            kinds.extend(FluxKind::SOURCES);
        }
        if self.kind.is_source() {
            kinds.extend([
                FluxKind::Kustomization,
                FluxKind::HelmRelease,
                FluxKind::HelmChart,
                FluxKind::ImageUpdateAutomation,
            ]);
        }
        kinds.sort();
        kinds.dedup();
        let served: Vec<FluxKind> = kinds
            .iter()
            .copied()
            .filter(|k| state::gvr(self.cluster(), *k, cx).is_some())
            .collect();
        let current: Vec<FluxKind> = self.related.iter().map(|(k, _)| *k).collect();
        if served == current {
            return;
        }
        self.related = kinds
            .into_iter()
            .filter_map(|kind| {
                let gvr = state::gvr(self.cluster(), kind, cx)?;
                Some((
                    kind,
                    ResourceStores::acquire(cx, state::all_key(self.cluster(), &gvr)),
                ))
            })
            .collect();
        self._observers = self
            .related
            .iter()
            .map(|(_, h)| cx.observe(h.entity(), |_, _, cx| cx.notify()))
            .collect();
    }

    fn refresh(&mut self, cx: &mut Context<Self>) {
        let key = kubyl_resources::object_key(self.target.namespace.as_deref(), self.name());
        let store = self.own.read(cx);
        let object = store
            .get(&key)
            .and_then(|o| FluxObject::parse_as(self.kind, o));
        self.gone = object.is_none() && store.status().is_ready();
        if let Some(object) = object {
            let changed = self
                .object
                .as_ref()
                .is_none_or(|o| !Arc::ptr_eq(&o.raw, &object.raw) && o.raw != object.raw);
            if changed {
                self.inventory = inventory::entries(&object.raw).map(Arc::new);
            }
            self.object = Some(object);
            self.publish_selection(cx);
        }
        if self.tab == Tab::Events || self.events.is_some() {
            self.ensure_events(cx);
        }
        if self.tab == Tab::Inventory {
            self.ensure_inventory(cx);
        }
        cx.notify();
    }

    /// The details dock and the actions follow the tab's object, while the tab has focus.
    fn publish_selection(&self, cx: &mut App) {
        if !self.focused {
            return;
        }
        let Some(object) = &self.object else {
            return;
        };
        ResourceSelection::set(
            cx,
            ResourceSelection {
                items: vec![Selected {
                    target: self.target.clone(),
                    kind: self.kind.kind().to_string(),
                    object: Some(object.raw.clone()),
                    store: Some(self.own.entity().clone()),
                }],
                caps: state::cluster_caps(self.cluster(), cx),
            },
        );
    }

    fn set_tab(&mut self, tab: Tab, cx: &mut Context<Self>) {
        self.tab = tab;
        self.refresh(cx);
    }

    // ----- Related objects -----

    fn loaded(&self, kind: FluxKind, cx: &App) -> Arc<Vec<FluxObject>> {
        self.related
            .iter()
            .find(|(k, _)| *k == kind)
            .map(|(k, h)| self.parsed.of(*k, h.entity(), cx))
            .unwrap_or_default()
    }

    fn find(&self, reference: &ObjectRef, cx: &App) -> Option<FluxObject> {
        let kind = reference.kind?;
        let (_, handle) = self.related.iter().find(|(k, _)| *k == kind)?;
        let key = kubyl_resources::object_key(Some(&reference.namespace), &reference.name);
        FluxObject::parse_as(kind, handle.read(cx).get(&key)?)
    }

    fn dependencies(&self, cx: &App) -> (Vec<Dependency>, Option<Dependency>, Vec<String>) {
        let Some(object) = &self.object else {
            return (Vec::new(), None, Vec::new());
        };
        let same = self.loaded(self.kind, cx);
        let graph = Graph::new(same.iter());
        let key = object.key();
        if same.is_empty() {
            // Not loaded yet: the dependencies by name, without states, and no verdict.
            return (Graph::new([object]).dependencies(&key), None, Vec::new());
        }
        (
            graph.dependencies(&key),
            graph.waiting(object),
            graph.dependents(&key),
        )
    }

    /// The objects that use this source.
    fn used_by(&self, cx: &App) -> Vec<FluxObject> {
        let Some(object) = &self.object else {
            return Vec::new();
        };
        let me = object.as_ref();
        let mut users: Vec<FluxObject> = [
            FluxKind::Kustomization,
            FluxKind::HelmRelease,
            FluxKind::HelmChart,
            FluxKind::ImageUpdateAutomation,
        ]
        .into_iter()
        .flat_map(|k| self.loaded(k, cx).to_vec())
        .filter(|o| {
            o.source
                .as_ref()
                .is_some_and(|s| s.kind == me.kind && s.key() == me.key())
        })
        .collect();
        users.sort_by_key(|o| (o.kind, o.key()));
        users
    }

    // ----- Events -----

    fn ensure_events(&mut self, cx: &mut Context<Self>) {
        if self.events.is_some() {
            return;
        }
        let key = StoreKey::new(
            self.cluster().clone(),
            Gvr::new("", "v1", "events"),
            self.target.namespace.clone(),
        )
        .fields(format!(
            "involvedObject.kind={},involvedObject.name={}",
            self.kind.kind(),
            self.name()
        ));
        let handle = ResourceStores::acquire(cx, key);
        self._subscriptions
            .push(cx.observe(handle.entity(), |_, _, cx| cx.notify()));
        self.events = Some(handle);
    }

    fn events(&self, cx: &App) -> Vec<Arc<Value>> {
        let Some(events) = &self.events else {
            return Vec::new();
        };
        let mut rows: Vec<Arc<Value>> = events.read(cx).objects().values().cloned().collect();
        rows.sort_by_key(|e| std::cmp::Reverse(kubyl_resources::columns::event_time(e)));
        rows
    }

    // ----- Inventory -----

    fn entries(&self) -> Option<Arc<Vec<Entry>>> {
        self.inventory.clone()
    }

    /// The served GVR of an inventory entry's kind.
    fn entry_gvr(&self, group: &str, kind: &str, cx: &App) -> Option<Gvr> {
        state::served_gvr(self.cluster(), group, kind, cx)
    }

    /// Acquires the stores the shown part of the inventory needs (and the children of its
    /// workloads: ReplicaSets and Pods). Lazy: only for the shown page. Kinds without a
    /// computed health are watched as metadata only, and a kind spread over many namespaces
    /// gets one watch in all namespaces.
    fn ensure_inventory(&mut self, cx: &mut Context<Self>) {
        let Some(entries) = self.entries() else {
            return;
        };
        // gvr → (full object?, namespaces).
        let mut wanted: HashMap<Gvr, (bool, BTreeSet<Option<String>>)> = HashMap::new();
        let mut want = |gvr: Gvr, full: bool, namespace: Option<String>| {
            let slot = wanted.entry(gvr).or_default();
            slot.0 |= full;
            slot.1.insert(namespace);
        };
        for entry in entries.iter().take(self.inventory_shown) {
            let Some(gvr) = self.entry_gvr(&entry.group, &entry.kind, cx) else {
                continue;
            };
            let namespace = entry.namespace().map(str::to_string);
            want(
                gvr,
                needs_full_object(&entry.group, &entry.kind),
                namespace.clone(),
            );
            let children: &[(&str, &str)] = match entry.kind.as_str() {
                "Deployment" => &[("apps", "ReplicaSet"), ("", "Pod")],
                "StatefulSet" | "DaemonSet" | "Job" => &[("", "Pod")],
                "CronJob" => &[("batch", "Job"), ("", "Pod")],
                _ => &[],
            };
            for (group, kind) in children {
                if let Some(gvr) = self.entry_gvr(group, kind, cx) {
                    want(gvr, true, namespace.clone());
                }
            }
        }
        let mut keys: Vec<(Gvr, Option<String>, bool)> = Vec::new();
        for (gvr, (full, namespaces)) in wanted {
            if namespaces.len() > INVENTORY_NAMESPACES {
                keys.push((gvr, None, full));
            } else {
                keys.extend(namespaces.into_iter().map(|ns| (gvr.clone(), ns, full)));
            }
        }
        for (gvr, namespace, full) in keys {
            if self
                .inventory_stores
                .contains_key(&(gvr.clone(), namespace.clone()))
            {
                continue;
            }
            let mut key = StoreKey::new(self.cluster().clone(), gvr.clone(), namespace.clone());
            if !full {
                key = key.metadata();
            }
            let handle = ResourceStores::acquire(cx, key);
            self._inventory_observers
                .push(cx.observe(handle.entity(), |_, _, cx| cx.notify()));
            self.inventory_stores.insert((gvr, namespace), handle);
        }
    }

    /// The store of `gvr` in `namespace`, else the kind's all-namespaces one.
    fn store_for(&self, gvr: &Gvr, namespace: Option<&str>) -> Option<&StoreHandle> {
        self.inventory_stores
            .get(&(gvr.clone(), namespace.map(str::to_string)))
            .or_else(|| self.inventory_stores.get(&(gvr.clone(), None)))
    }

    /// The Events and inventory stores "Ask agent" reads have loaded (or failed).
    fn agent_inputs_loaded(&self, cx: &App) -> bool {
        self.events
            .iter()
            .chain(self.inventory_stores.values())
            .all(|h| {
                !matches!(
                    h.read(cx).status(),
                    StoreStatus::Waiting | StoreStatus::Loading
                )
            })
    }

    /// The inventory as rows: each entry, then (unless collapsed) its children by owner
    /// references (Deployment → ReplicaSet → Pod). The children's stores are indexed by owner
    /// once per call, so a large inventory over many Pods costs one pass over them.
    fn inventory_nodes(&self, cx: &App) -> Vec<Node> {
        let Some(entries) = self.entries() else {
            return Vec::new();
        };
        let mut gvrs: HashMap<(&str, &str), Option<Gvr>> = HashMap::new();
        let mut gvr_of = |group: &'static str, kind: &'static str| -> Option<Gvr> {
            gvrs.entry((group, kind))
                .or_insert_with(|| self.entry_gvr(group, kind, cx))
                .clone()
        };
        let owners = self.owner_index(&mut gvr_of, cx);
        let mut entry_gvrs: HashMap<(String, String), Option<Gvr>> = HashMap::new();
        let mut nodes = Vec::new();
        for entry in entries.iter().take(self.inventory_shown) {
            let gvr = entry_gvrs
                .entry((entry.group.clone(), entry.kind.clone()))
                .or_insert_with(|| self.entry_gvr(&entry.group, &entry.kind, cx))
                .clone();
            let live = gvr.as_ref().and_then(|gvr| {
                let store = self.store_for(gvr, entry.namespace())?.read(cx);
                let object = store
                    .get(&kubyl_resources::object_key(entry.namespace(), &entry.name))
                    .cloned();
                Some((object, store.status().is_ready()))
            });
            let health = match &live {
                Some((Some(object), _)) => {
                    Some(inventory::health(&entry.group, &entry.kind, object))
                }
                Some((None, true)) => Some(NodeHealth::missing()),
                _ => None,
            };
            let key = format!("{}/{}/{}", entry.kind, entry.namespace, entry.name);
            let uid = live
                .as_ref()
                .and_then(|(o, _)| o.as_ref())
                .and_then(|o| o.pointer("/metadata/uid").and_then(Value::as_str))
                .map(str::to_string);
            let children = uid
                .map(|uid| self.children(&owners, &uid, entry.namespace(), 1))
                .unwrap_or_default();
            nodes.push(Node {
                depth: 0,
                kind: entry.kind.clone(),
                name: entry.name.clone(),
                namespace: entry.namespace().map(str::to_string),
                gvr,
                health,
                has_children: !children.is_empty(),
                key: key.clone(),
            });
            if !self.collapsed.contains(&key) {
                nodes.extend(children);
            }
        }
        nodes
    }

    /// The loaded ReplicaSets, Jobs and Pods by the uid of their owner, sorted by name.
    fn owner_index(
        &self,
        gvr_of: &mut dyn FnMut(&'static str, &'static str) -> Option<Gvr>,
        cx: &App,
    ) -> HashMap<String, Vec<Child>> {
        let mut owners: HashMap<String, Vec<Child>> = HashMap::new();
        for (group, kind) in [("apps", "ReplicaSet"), ("batch", "Job"), ("", "Pod")] {
            let Some(gvr) = gvr_of(group, kind) else {
                continue;
            };
            for ((store_gvr, _), handle) in &self.inventory_stores {
                if *store_gvr != gvr {
                    continue;
                }
                for object in handle.read(cx).objects().values() {
                    let Some(refs) = object
                        .pointer("/metadata/ownerReferences")
                        .and_then(Value::as_array)
                    else {
                        continue;
                    };
                    for owner in refs.iter().filter_map(|r| r["uid"].as_str()) {
                        owners.entry(owner.to_string()).or_default().push(Child {
                            group,
                            kind,
                            gvr: gvr.clone(),
                            object: object.clone(),
                        });
                    }
                }
            }
        }
        let name = |c: &Child| {
            c.object
                .pointer("/metadata/name")
                .and_then(Value::as_str)
                .unwrap_or_default()
                .to_string()
        };
        for children in owners.values_mut() {
            children.sort_by_key(|c| (c.kind != "ReplicaSet", c.kind != "Job", name(c)));
            // The same object from an all-namespaces and a namespaced watch.
            children.dedup_by(|a, b| a.kind == b.kind && name(a) == name(b));
        }
        owners
    }

    /// Objects owned by `uid` (from [`Self::owner_index`]), with theirs below them.
    fn children(
        &self,
        owners: &HashMap<String, Vec<Child>>,
        uid: &str,
        namespace: Option<&str>,
        depth: usize,
    ) -> Vec<Node> {
        if depth > 3 {
            return Vec::new();
        }
        let mut nodes = Vec::new();
        for child in owners.get(uid).into_iter().flatten() {
            let object = &child.object;
            // Old ReplicaSets scaled to 0 stay out of the way.
            if child.kind == "ReplicaSet"
                && object.pointer("/spec/replicas").and_then(Value::as_i64) == Some(0)
            {
                continue;
            }
            let name = object
                .pointer("/metadata/name")
                .and_then(Value::as_str)
                .unwrap_or_default()
                .to_string();
            let child_uid = object
                .pointer("/metadata/uid")
                .and_then(Value::as_str)
                .unwrap_or_default();
            let key = format!("{}/{}/{name}", child.kind, namespace.unwrap_or_default());
            let grandchildren = self.children(owners, child_uid, namespace, depth + 1);
            nodes.push(Node {
                depth,
                kind: child.kind.to_string(),
                name,
                namespace: namespace.map(str::to_string),
                gvr: Some(child.gvr.clone()),
                health: Some(inventory::health(child.group, child.kind, object)),
                has_children: !grandchildren.is_empty(),
                key: key.clone(),
            });
            if !self.collapsed.contains(&key) {
                nodes.extend(grandchildren);
            }
        }
        nodes
    }

    /// Inventory objects that aren't healthy (for "Ask agent").
    fn unhealthy(&self, cx: &App) -> Vec<Unhealthy> {
        self.inventory_nodes(cx)
            .into_iter()
            .filter_map(|n| {
                let health = n.health?;
                (!health.is_healthy()).then(|| Unhealthy {
                    label: match &n.namespace {
                        Some(ns) => format!("{} {ns}/{}", n.kind, n.name),
                        None => format!("{} {}", n.kind, n.name),
                    },
                    status: format!("{} {}", health.label, health.info)
                        .trim()
                        .to_string(),
                })
            })
            .collect()
    }

    fn open_node(&self, node: &Node, window: &mut Window, cx: &mut App) {
        let Some(gvr) = node.gvr.clone() else {
            return;
        };
        let target = ResourceRef::object(
            self.cluster().clone(),
            gvr.clone(),
            node.namespace.clone(),
            node.name.clone(),
        );
        let kind = ViewRegistry::object_view(cx, &gvr);
        window.dispatch_action(
            Box::new(OpenView(ViewRequest::for_resource(kind, target))),
            cx,
        );
    }

    // ----- Actions -----

    fn writable(&self, cx: &App) -> bool {
        !state::read_only(self.cluster(), cx) && !self.gone
    }

    fn act(&self, action: Action, window: &mut Window, cx: &mut App) {
        actions::request(vec![self.target.clone()], action, window, cx);
    }

    /// Loads the Events and the inventory (if their tabs weren't opened), waits for them (at
    /// most [`AGENT_WAIT`]), then asks the agent.
    fn ask_agent(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.object.is_none() {
            return;
        }
        self.ensure_events(cx);
        self.ensure_inventory(cx);
        self._ask = Some(cx.spawn_in(window, async move |this, cx| {
            let started = std::time::Instant::now();
            loop {
                let loaded = this
                    .update(cx, |this, cx| this.agent_inputs_loaded(cx))
                    .unwrap_or(true);
                if loaded || started.elapsed() >= AGENT_WAIT {
                    break;
                }
                cx.background_executor()
                    .timer(std::time::Duration::from_millis(100))
                    .await;
            }
            this.update_in(cx, |this, window, cx| this.send_to_agent(window, cx))
                .ok();
        }));
    }

    fn send_to_agent(&self, window: &mut Window, cx: &mut App) {
        let Some(object) = &self.object else {
            return;
        };
        let (deps, _, _) = self.dependencies(cx);
        let now = jiff::Timestamp::now();
        let warnings: Vec<Warning> = self
            .events(cx)
            .iter()
            .filter(|e| e["type"].as_str() == Some("Warning"))
            .map(|e| Warning {
                reason: e["reason"].as_str().unwrap_or_default().to_string(),
                message: kubyl_resources::columns::event_message(e).to_string(),
                age: kubyl_resources::columns::event_time(e)
                    .map(|t| {
                        kubyl_resources::format::human_duration(
                            kubyl_resources::format::seconds_since(t, now),
                        )
                    })
                    .unwrap_or_default(),
                count: e["count"].as_i64().unwrap_or(1),
            })
            .collect();
        let text = agent::text(object, &deps, &self.unhealthy(cx), &warnings);
        window.dispatch_action(
            Box::new(AskAgent {
                cluster: self.cluster().clone(),
                label: format!("Flux {} {}", object.kind, object.key()),
                uri: format!(
                    "kubyl://flux/{}/{}/{}",
                    object.kind.plural(),
                    object.namespace,
                    object.name
                ),
                text,
            }),
            cx,
        );
    }

    /// Kind and name of the header, for tests.
    pub fn state(&self) -> Option<State> {
        self.object.as_ref().map(FluxObject::state)
    }

    // ----- Rendering -----

    fn render_header(&self, cx: &mut Context<Self>) -> AnyElement {
        let colors = cx.colors().clone();
        let Some(object) = self.object.clone() else {
            return div().into_any_element();
        };
        let writable = self.writable(cx);
        let state = object.state();
        let entity = cx.entity().downgrade();
        let reconcile = {
            let weak = entity.clone();
            Button::new("flux-reconcile")
                .primary()
                .icon(IconName::RefreshCw)
                .label("Reconcile")
                .disabled(!Action::Reconcile.applies_to(&object))
                .on_click(move |_, window, cx| {
                    weak.update(cx, |this, cx| this.act(Action::Reconcile, window, cx))
                        .ok();
                })
        };
        let reconcile_more = {
            let weak = entity.clone();
            let object = object.clone();
            MenuButton::new("flux-reconcile-more")
                .ghost()
                .compact()
                .child(Icon::new(IconName::ChevronDown).size(12.0))
                .dropdown_menu(move |menu, _, _| {
                    let mut menu = menu;
                    for action in [Action::ReconcileWithSource, Action::Force, Action::Reset] {
                        if !action.applies_to(&object) {
                            continue;
                        }
                        let weak = weak.clone();
                        menu = menu.item(PopupMenuItem::new(action.label()).on_click(
                            move |_, window, cx| {
                                weak.update(cx, |this, cx| this.act(action, window, cx))
                                    .ok();
                            },
                        ));
                    }
                    menu
                })
        };
        let suspend = {
            let weak = entity.clone();
            let (action, icon) = if object.suspended {
                (Action::Resume, IconName::Play)
            } else {
                (Action::Suspend, IconName::Pause)
            };
            Button::new("flux-suspend")
                .icon(icon)
                .label(action.label())
                .disabled(!action.applies_to(&object))
                .on_click(move |_, window, cx| {
                    weak.update(cx, |this, cx| this.act(action, window, cx))
                        .ok();
                })
        };
        let ask = {
            let weak = entity.clone();
            Button::new("flux-ask")
                .ghost()
                .icon(IconName::Zap)
                .label("Ask agent")
                .on_click(move |_, window, cx| {
                    weak.update(cx, |this, cx| this.ask_agent(window, cx)).ok();
                })
                .disabled(self._ask.is_some() && !self.agent_inputs_loaded(cx))
        };
        let more = {
            let target = self.target.clone();
            MenuButton::new("flux-more")
                .ghost()
                .compact()
                .child(Icon::new(IconName::Ellipsis).size(14.0))
                .dropdown_menu(move |menu, _, _| {
                    let edit = target.clone();
                    let logs = target.clone();
                    let delete = target.clone();
                    let mut menu = menu
                        .item(
                            PopupMenuItem::new("Edit YAML").on_click(move |_, window, cx| {
                                actions::edit_yaml(edit.clone(), window, cx)
                            }),
                        )
                        .item(PopupMenuItem::new("Controller Logs").on_click(
                            move |_, window, cx| actions::controller_logs(&logs, window, cx),
                        ));
                    if writable {
                        menu = menu
                            .separator()
                            .item(
                                PopupMenuItem::new("Delete…").on_click(move |_, window, cx| {
                                    actions::request(
                                        vec![delete.clone()],
                                        Action::Delete,
                                        window,
                                        cx,
                                    )
                                }),
                            );
                    }
                    menu
                })
        };
        h_flex()
            .flex_none()
            .px(u(16.0))
            .pt(u(12.0))
            .pb(u(10.0))
            .gap(u(10.0))
            .border_b_1()
            .border_color(colors.border_variant)
            .child(
                div()
                    .flex_none()
                    .size(u(30.0))
                    .rounded(u(7.0))
                    .flex()
                    .items_center()
                    .justify_center()
                    .bg(colors.accent.opacity(0.13))
                    .border_1()
                    .border_color(colors.accent.opacity(0.33))
                    .child(
                        Icon::new(widgets::kind_icon(object.kind))
                            .size(16.0)
                            .color(colors.accent),
                    ),
            )
            .child(
                v_flex()
                    .min_w_0()
                    .child(
                        h_flex()
                            .gap(u(8.0))
                            .child(
                                div()
                                    .font_family(fonts::MONO)
                                    .text_size(u(15.0))
                                    .font_weight(FontWeight::MEDIUM)
                                    .child(object.name.clone()),
                            )
                            .child(Chip::new(object.namespace.clone()))
                            .child(
                                div()
                                    .text_size(u(12.0))
                                    .text_color(colors.text_dim)
                                    .child(format!("{} · {}", object.kind, object.api_version)),
                            ),
                    )
                    .child(
                        h_flex()
                            .mt(u(3.0))
                            .gap(u(12.0))
                            .text_size(u(12.0))
                            .child(widgets::state_pill(state, &colors))
                            .when_some(object.revision(), |this, r| {
                                this.child(
                                    div()
                                        .font_family(fonts::MONO)
                                        .text_size(u(11.5))
                                        .text_color(colors.text_muted)
                                        .child(short_revision(&r)),
                                )
                            })
                            .when_some(object.interval.clone(), |this, i| {
                                this.child(
                                    div()
                                        .text_color(colors.text_dim)
                                        .child(format!("every {i}")),
                                )
                            }),
                    ),
            )
            .child(div().flex_1())
            .when(writable && object.reconcilable(), |this| {
                this.child(h_flex().child(reconcile).child(reconcile_more))
            })
            .when(writable && object.suspendable(), |this| this.child(suspend))
            .child(ask)
            .child(more)
            .into_any_element()
    }

    fn render_tabs(&self, cx: &mut Context<Self>) -> AnyElement {
        let colors = cx.colors().clone();
        let object = self.object.clone();
        let counts = |tab: Tab| -> Option<String> {
            let object = object.as_ref()?;
            match tab {
                Tab::Inventory => self.inventory.as_ref().map(|e| e.len().to_string()),
                Tab::History => {
                    let n = match object.kind {
                        FluxKind::HelmRelease => helm_history(object).len(),
                        _ => kustomization_history(object).len(),
                    };
                    (n > 0).then(|| n.to_string())
                }
                _ => None,
            }
        };
        h_flex()
            .flex_none()
            .h(u(36.0))
            .px(u(12.0))
            .gap(u(2.0))
            .border_b_1()
            .border_color(colors.border_variant)
            .children(Tab::of(self.kind).into_iter().map(|tab| {
                let active = tab == self.tab;
                h_flex()
                    .id(SharedString::from(format!("flux-tab-{}", tab.label())))
                    .h_full()
                    .px(u(10.0))
                    .gap(u(6.0))
                    .cursor_pointer()
                    .text_size(u(12.5))
                    .text_color(if active { colors.text } else { colors.text_dim })
                    .when(active, |this| this.border_b_2().border_color(colors.accent))
                    .on_click(cx.listener(move |this, _, _, cx| this.set_tab(tab, cx)))
                    .child(tab.label())
                    .when_some(counts(tab), |this, n| this.child(Chip::new(n)))
            }))
            .into_any_element()
    }

    fn render_summary(&self, cx: &mut Context<Self>) -> AnyElement {
        let colors = cx.colors().clone();
        let Some(object) = self.object.clone() else {
            return widgets::empty("Loading…", &colors);
        };
        let state = object.state();
        let message = object.message();
        let status = {
            let boxed = match state {
                State::Failed | State::Stalled => widgets::error_box(&colors),
                State::Suspended | State::Unknown | State::Reconciling => {
                    widgets::warning_box(&colors)
                }
                State::Ready => h_flex().items_start().gap(u(10.0)).p(u(2.0)),
            };
            boxed
                .mx(u(14.0))
                .mt(u(12.0))
                .child(
                    Icon::new(match state {
                        State::Ready => IconName::CircleCheck,
                        State::Failed | State::Stalled => IconName::CircleX,
                        State::Suspended => IconName::Pause,
                        _ => IconName::RefreshCw,
                    })
                    .size(15.0)
                    .color(widgets::state_color(state, &colors)),
                )
                .child(
                    v_flex()
                        .min_w_0()
                        .flex_1()
                        .gap(u(2.0))
                        .child(
                            div()
                                .text_size(u(12.5))
                                .font_weight(FontWeight::SEMIBOLD)
                                .text_color(widgets::state_color(state, &colors))
                                .child(state.label()),
                        )
                        .child(
                            div()
                                .text_size(u(12.0))
                                .text_color(colors.text_muted)
                                .child(message),
                        ),
                )
        };
        let (deps, waiting, dependents) = self.dependencies(cx);
        let left = v_flex()
            .flex_1()
            .min_w_0()
            .child(status)
            .child(self.render_conditions(&object, &colors))
            .when(!deps.is_empty() || !dependents.is_empty(), |this| {
                this.child(self.render_dependencies(
                    &deps,
                    waiting.as_ref(),
                    &dependents,
                    &colors,
                    cx,
                ))
            })
            .when(object.kind == FluxKind::HelmRelease, |this| {
                this.child(self.render_releases(&object, &colors, cx))
            })
            .when(object.kind.is_source(), |this| {
                this.child(self.render_used_by(&colors, cx))
            });
        let right = v_flex()
            .w(u(380.0))
            .flex_none()
            .border_l_1()
            .border_color(colors.border_variant)
            .bg(colors.panel)
            .children(match object.kind {
                FluxKind::Kustomization => vec![
                    self.render_source(&object, &colors, cx),
                    self.render_kustomization(&object, &colors),
                ],
                FluxKind::HelmRelease => vec![
                    self.render_chart(&object, &colors, cx),
                    self.render_values(&object, &colors),
                    self.render_remediation(&object, &colors),
                ],
                _ => vec![self.render_facts(&object, &colors)],
            });
        h_flex()
            .id("flux-summary")
            .flex_1()
            .min_h_0()
            .items_start()
            .overflow_y_scroll()
            .child(left)
            .child(right)
            .into_any_element()
    }

    /// The latest Helm release revisions, each linking to its Helm release tab.
    fn render_releases(
        &self,
        object: &FluxObject,
        colors: &Colors,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let history = helm_history(object);
        let storage_ns = helm_storage(object).map(|(ns, _)| ns);
        let mut section = widgets::section(format!("Releases · {}", history.len()), colors);
        if history.is_empty() {
            section = section.child(
                div()
                    .text_size(u(12.0))
                    .text_color(colors.text_dim)
                    .child("No release history in the status yet."),
            );
        }
        for (index, entry) in history.iter().take(5).enumerate() {
            let cluster = self.cluster().clone();
            let namespace = storage_ns
                .clone()
                .unwrap_or_else(|| entry.namespace.clone());
            let secret = entry.storage_secret();
            let color = match entry.status.as_str() {
                "deployed" => colors.green,
                "failed" => colors.red,
                _ => colors.text_dim,
            };
            section = section.child(
                h_flex()
                    .gap(u(10.0))
                    .text_size(u(12.0))
                    .child(
                        div()
                            .w(u(32.0))
                            .child(widgets::mono(format!("v{}", entry.version))),
                    )
                    .child(div().flex_1().min_w_0().child(widgets::mono(format!(
                        "{} {}",
                        entry.chart_name, entry.chart_version
                    ))))
                    .child(
                        div()
                            .w(u(100.0))
                            .child(widgets::pill(entry.status.clone(), color)),
                    )
                    .child(
                        div()
                            .w(u(40.0))
                            .child(widgets::mono(widgets::age(entry.last_deployed.as_deref()))),
                    )
                    .child(widgets::link(
                        ("flux-release", index),
                        "Helm release",
                        colors,
                        move |_, window, cx| {
                            open_helm_release(&cluster, &namespace, &secret, window, cx)
                        },
                    )),
            );
        }
        let _ = cx;
        section.into_any_element()
    }

    fn render_conditions(&self, object: &FluxObject, colors: &Colors) -> AnyElement {
        let mut section =
            widgets::section(format!("Conditions · {}", object.conditions.len()), colors);
        if object.conditions.is_empty() {
            section = section.child(div().text_size(u(12.0)).text_color(colors.text_dim).child(
                if object.is_static() {
                    "Static object: no controller reconciles it, so it has no status."
                } else {
                    "No conditions yet: the controller hasn't reconciled it."
                },
            ));
        }
        for (index, condition) in object.conditions.iter().enumerate() {
            let (icon, color) = match (condition.kind.as_str(), condition.status.as_str()) {
                ("Ready", "True") | (_, "True")
                    if condition.kind != "Stalled" && condition.kind != "Reconciling" =>
                {
                    (IconName::CircleCheck, colors.green)
                }
                ("Reconciling", "True") => (IconName::RefreshCw, colors.accent),
                ("Stalled", "True") => (IconName::CircleX, colors.orange),
                (_, "False") => (IconName::CircleX, colors.red),
                _ => (IconName::Info, colors.text_dim),
            };
            section =
                section.child(
                    h_flex()
                        .id(("flux-condition", index))
                        .items_start()
                        .gap(u(8.0))
                        .text_size(u(12.0))
                        .child(Icon::new(icon).size(13.0).color(color))
                        .child(
                            v_flex()
                                .min_w_0()
                                .flex_1()
                                .child(
                                    h_flex()
                                        .gap(u(6.0))
                                        .child(div().font_weight(FontWeight::MEDIUM).child(
                                            format!("{}={}", condition.kind, condition.status),
                                        ))
                                        .child(
                                            div()
                                                .text_color(colors.text_dim)
                                                .child(condition.reason.clone()),
                                        )
                                        .child(div().flex_1())
                                        .child(
                                            div()
                                                .font_family(fonts::MONO)
                                                .text_size(u(11.0))
                                                .text_color(colors.text_dim)
                                                .child(widgets::age(
                                                    condition.last_transition.as_deref(),
                                                )),
                                        ),
                                )
                                .child(
                                    div()
                                        .text_color(colors.text_muted)
                                        .child(condition.message.clone()),
                                ),
                        ),
                );
        }
        section.into_any_element()
    }

    fn render_dependencies(
        &self,
        deps: &[Dependency],
        waiting: Option<&Dependency>,
        dependents: &[String],
        colors: &Colors,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let mut section = widgets::section(format!("Dependencies · {}", deps.len()), colors);
        if let Some(waiting) = waiting {
            section = section.child(
                widgets::warning_box(colors)
                    .text_size(u(12.0))
                    .child(Icon::new(IconName::Clock).size(13.0).color(colors.yellow))
                    .child(format!(
                        "Waiting for {} ({})",
                        waiting.key,
                        waiting.state.map_or("not found", |s| s.label())
                    )),
            );
        }
        for (index, dep) in deps.iter().enumerate() {
            let (ns, name) = dep.key.split_once('/').unwrap_or(("", &dep.key));
            let target = self.sibling(ns, name, cx);
            section = section.child(
                h_flex()
                    .gap(u(8.0))
                    .text_size(u(12.0))
                    .child(
                        div()
                            .text_color(colors.text_dim)
                            .w(u(70.0))
                            .child("depends on"),
                    )
                    .child(match target {
                        Some(target) => widgets::link(
                            ("flux-dep", index),
                            dep.key.clone(),
                            colors,
                            move |_, window, cx| actions::open_object(target.clone(), window, cx),
                        )
                        .font_family(fonts::MONO)
                        .into_any_element(),
                        None => widgets::mono(dep.key.clone()),
                    })
                    .child(div().flex_1())
                    .child(match dep.state {
                        Some(state) => widgets::state_pill(state, colors).into_any_element(),
                        None => widgets::pill("Not found", colors.yellow).into_any_element(),
                    }),
            );
        }
        for (index, key) in dependents.iter().enumerate() {
            let (ns, name) = key.split_once('/').unwrap_or(("", key));
            let target = self.sibling(ns, name, cx);
            section = section.child(
                h_flex()
                    .gap(u(8.0))
                    .text_size(u(12.0))
                    .child(
                        div()
                            .text_color(colors.text_dim)
                            .w(u(70.0))
                            .child("needed by"),
                    )
                    .child(match target {
                        Some(target) => widgets::link(
                            ("flux-dependent", index),
                            key.clone(),
                            colors,
                            move |_, window, cx| actions::open_object(target.clone(), window, cx),
                        )
                        .font_family(fonts::MONO)
                        .into_any_element(),
                        None => widgets::mono(key.clone()),
                    }),
            );
        }
        section.into_any_element()
    }

    /// The ref of an object of the same kind.
    fn sibling(&self, namespace: &str, name: &str, _cx: &App) -> Option<ResourceRef> {
        Some(ResourceRef::object(
            self.cluster().clone(),
            self.target.gvr.clone(),
            Some(namespace.to_string()),
            name.to_string(),
        ))
    }

    fn ref_of(&self, reference: &ObjectRef, cx: &App) -> Option<ResourceRef> {
        let gvr = state::gvr(self.cluster(), reference.kind?, cx)?;
        Some(ResourceRef::object(
            self.cluster().clone(),
            gvr,
            Some(reference.namespace.clone()),
            reference.name.clone(),
        ))
    }

    fn render_used_by(&self, colors: &Colors, cx: &mut Context<Self>) -> AnyElement {
        let users = self.used_by(cx);
        let mut section = widgets::section(format!("Used by · {}", users.len()), colors);
        if users.is_empty() {
            section = section.child(
                div()
                    .text_size(u(12.0))
                    .text_color(colors.text_dim)
                    .child("Nothing uses this source."),
            );
        }
        for (index, user) in users.iter().enumerate() {
            let target = actions::object_ref(self.cluster(), user, cx);
            section = section.child(
                h_flex()
                    .gap(u(8.0))
                    .text_size(u(12.0))
                    .child(
                        div()
                            .w(u(110.0))
                            .text_color(colors.text_dim)
                            .child(user.kind.kind()),
                    )
                    .child(match target {
                        Some(target) => widgets::link(
                            ("flux-user", index),
                            user.key(),
                            colors,
                            move |_, window, cx| actions::open_object(target.clone(), window, cx),
                        )
                        .font_family(fonts::MONO)
                        .into_any_element(),
                        None => widgets::mono(user.key()),
                    })
                    .child(div().flex_1())
                    .child(widgets::state_pill(user.state(), colors)),
            );
        }
        section.into_any_element()
    }

    /// The source with its revision (a commit link for GitHub/GitLab/Bitbucket).
    fn render_source(
        &self,
        object: &FluxObject,
        colors: &Colors,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let mut rows: Vec<(&'static str, AnyElement)> = Vec::new();
        if let Some(source) = &object.source {
            let label = source.label(&object.namespace);
            rows.push((
                "Source",
                match self.ref_of(source, cx) {
                    Some(target) => {
                        widgets::link("flux-source", label, colors, move |_, window, cx| {
                            actions::open_object(target.clone(), window, cx)
                        })
                        .font_family(fonts::MONO)
                        .text_size(u(11.5))
                        .into_any_element()
                    }
                    None => widgets::kv_mono(label),
                },
            ));
            let source_object = self.find(source, cx);
            if let Some(url) = source_object.as_ref().and_then(details::source_url) {
                rows.push(("URL", widgets::kv_mono(url)));
            }
            if let Some(revision) = &object.last_applied_revision {
                let repo = source_object.as_ref().and_then(|s| {
                    s.artifact
                        .as_ref()
                        .and_then(|a| a.source_url.clone())
                        .or_else(|| details::source_url(s))
                });
                let commit = commit_of(revision).or_else(|| {
                    source_object.as_ref().and_then(|s| {
                        s.artifact
                            .as_ref()?
                            .source_revision
                            .as_deref()
                            .and_then(commit_of)
                    })
                });
                let link = repo
                    .zip(commit)
                    .and_then(|(repo, sha)| links::commit_url(&repo, &sha));
                rows.push((
                    "Applied",
                    match link {
                        Some(url) => h_flex()
                            .gap(u(4.0))
                            .child(
                                widgets::url_link(
                                    "flux-commit",
                                    short_revision(revision),
                                    url,
                                    colors,
                                )
                                .font_family(fonts::MONO)
                                .text_size(u(11.5)),
                            )
                            .child(
                                Icon::new(IconName::ExternalLink)
                                    .size(11.0)
                                    .color(colors.text_dim),
                            )
                            .into_any_element(),
                        None => widgets::kv_mono(short_revision(revision)),
                    },
                ));
            }
            if let Some(attempted) = &object.last_attempted_revision
                && object.last_applied_revision.as_ref() != Some(attempted)
            {
                rows.push(("Attempted", widgets::kv_mono(short_revision(attempted))));
            }
            if let Some(artifact) = source_object.as_ref().and_then(|s| s.artifact.clone()) {
                rows.push((
                    "Source at",
                    widgets::kv_mono(short_revision(&artifact.revision)),
                ));
            }
        }
        widgets::section("Source", colors)
            .child(widgets::kv(rows, colors))
            .into_any_element()
    }

    fn render_kustomization(&self, object: &FluxObject, colors: &Colors) -> AnyElement {
        let spec = KustomizationSpec::parse(object);
        let mut rows: Vec<(&'static str, AnyElement)> = vec![
            ("Path", widgets::kv_mono(spec.path.clone())),
            (
                "Prune",
                widgets::kv_text(if spec.prune {
                    "on: removes what leaves Git"
                } else {
                    "off"
                }),
            ),
            (
                "Target ns",
                widgets::kv_mono(
                    spec.target_namespace
                        .clone()
                        .unwrap_or_else(|| "(as in the manifests)".into()),
                ),
            ),
        ];
        if let Some(interval) = &object.interval {
            rows.push(("Interval", widgets::kv_mono(interval.clone())));
        }
        if let Some(retry) = &spec.retry_interval {
            rows.push(("Retry", widgets::kv_mono(retry.clone())));
        }
        if let Some(timeout) = &spec.timeout {
            rows.push(("Timeout", widgets::kv_mono(timeout.clone())));
        }
        if spec.force {
            rows.push((
                "Force",
                widgets::kv_text("on: re-creates immutable objects"),
            ));
        }
        if spec.wait {
            rows.push((
                "Wait",
                widgets::kv_text("waits for all applied objects to be ready"),
            ));
        }
        if let Some(sa) = &spec.service_account {
            rows.push(("Account", widgets::kv_mono(sa.clone())));
        }
        if !spec.components.is_empty() {
            rows.push(("Components", widgets::kv_mono(spec.components.join(", "))));
        }
        let mut section =
            widgets::section("Kustomization", colors).child(widgets::kv(rows, colors));
        if !spec.substitutions.is_empty() || !spec.substitute_from.is_empty() {
            let mut subst = v_flex()
                .gap(u(4.0))
                .text_size(u(12.0))
                .child(widgets::title_line("Post-build substitutions", colors));
            for (name, value) in &spec.substitutions {
                subst = subst.child(
                    h_flex()
                        .gap(u(6.0))
                        .child(widgets::mono(format!("${{{name}}}")))
                        .child(div().text_color(colors.text_dim).child("="))
                        .child(widgets::mono(value.clone())),
                );
            }
            for data in &spec.substitute_from {
                subst = subst.child(data_ref_row(data, colors));
            }
            section = section.child(subst);
        }
        section.into_any_element()
    }

    fn render_chart(
        &self,
        object: &FluxObject,
        colors: &Colors,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let spec = HelmReleaseSpec::parse(object);
        let history = helm_history(object);
        let latest = history.first();
        let mut rows: Vec<(&'static str, AnyElement)> = vec![(
            "Chart",
            widgets::kv_mono(match &spec.version {
                Some(v) => format!("{} {v}", spec.chart),
                None => spec.chart.clone(),
            }),
        )];
        if let Some(latest) = latest {
            rows.push((
                "Installed",
                widgets::kv_mono(format!(
                    "{} {}{}",
                    latest.chart_name,
                    latest.chart_version,
                    latest
                        .app_version
                        .as_ref()
                        .map(|a| format!(" (app {a})"))
                        .unwrap_or_default()
                )),
            ));
        } else if let Some(revision) = &object.last_applied_revision {
            rows.push(("Installed", widgets::kv_mono(revision.clone())));
        }
        if let Some(source) = &spec.source {
            let label = source.label(&object.namespace);
            rows.push((
                if spec.chart_ref { "Chart ref" } else { "From" },
                match self.ref_of(source, cx) {
                    Some(target) => {
                        widgets::link("flux-chart-source", label, colors, move |_, window, cx| {
                            actions::open_object(target.clone(), window, cx)
                        })
                        .font_family(fonts::MONO)
                        .text_size(u(11.5))
                        .into_any_element()
                    }
                    None => widgets::kv_mono(label),
                },
            ));
            if let Some(url) = self.find(source, cx).as_ref().and_then(details::source_url) {
                rows.push(("URL", widgets::kv_mono(url)));
            }
        }
        rows.push(("Release", widgets::kv_mono(spec.release_name.clone())));
        if let Some(ns) = &spec.target_namespace {
            rows.push(("Target ns", widgets::kv_mono(ns.clone())));
        }
        let storage = helm_storage(object);
        let cluster = self.cluster().clone();
        widgets::section("Chart", colors)
            .child(widgets::kv(rows, colors))
            .when_some(storage, |this, (namespace, secret)| {
                this.child(
                    h_flex().child(
                        Button::new("flux-helm-release")
                            .ghost()
                            .icon(IconName::Anchor)
                            .label("Open Helm release")
                            .on_click(move |_, window, cx| {
                                open_helm_release(&cluster, &namespace, &secret, window, cx)
                            }),
                    ),
                )
            })
            .into_any_element()
    }

    fn render_values(&self, object: &FluxObject, colors: &Colors) -> AnyElement {
        let spec = HelmReleaseSpec::parse(object);
        let mut section = widgets::section("Values", colors);
        if spec.values_from.is_empty() && spec.inline_values.is_empty() {
            section = section.child(
                div()
                    .text_size(u(12.0))
                    .text_color(colors.text_dim)
                    .child("The chart's defaults."),
            );
        }
        for data in &spec.values_from {
            section = section.child(data_ref_row(data, colors));
        }
        if !spec.inline_values.is_empty() {
            section = section.child(
                h_flex()
                    .gap(u(6.0))
                    .text_size(u(12.0))
                    .child(div().text_color(colors.text_dim).child("inline"))
                    .child(widgets::mono(spec.inline_values.join(", "))),
            );
        }
        section.into_any_element()
    }

    fn render_remediation(&self, object: &FluxObject, colors: &Colors) -> AnyElement {
        let spec = HelmReleaseSpec::parse(object);
        let failures = failures(object);
        let mut rows: Vec<(&'static str, AnyElement)> = vec![
            (
                "Install",
                widgets::kv_text(HelmReleaseSpec::retries_label(spec.install_retries)),
            ),
            (
                "Upgrade",
                widgets::kv_text(HelmReleaseSpec::retries_label(spec.upgrade_retries)),
            ),
        ];
        if let Some(remediate) = spec.remediate_last_failure {
            rows.push((
                "Last failure",
                widgets::kv_text(if remediate {
                    "remediated"
                } else {
                    "left as is"
                }),
            ));
        }
        if let Some(strategy) = &spec.upgrade_strategy {
            rows.push(("Strategy", widgets::kv_mono(strategy.clone())));
        }
        if let Some(mode) = &spec.drift_detection {
            rows.push(("Drift", widgets::kv_mono(mode.clone())));
        }
        rows.push((
            "Failures",
            div()
                .text_color(
                    if failures.total + failures.install + failures.upgrade > 0 {
                        colors.red
                    } else {
                        colors.text
                    },
                )
                .child(format!(
                    "install {} · upgrade {} · total {}",
                    failures.install, failures.upgrade, failures.total
                ))
                .into_any_element(),
        ));
        widgets::section("Remediation", colors)
            .child(widgets::kv(rows, colors))
            .into_any_element()
    }

    fn render_facts(&self, object: &FluxObject, colors: &Colors) -> AnyElement {
        let mut rows: Vec<(&'static str, AnyElement)> = details::facts(object)
            .into_iter()
            .map(|f| {
                (
                    f.label,
                    if f.mono {
                        widgets::kv_mono(f.value)
                    } else {
                        widgets::kv_text(f.value)
                    },
                )
            })
            .collect();
        if let Some(artifact) = &object.artifact {
            rows.push(("Revision", widgets::kv_mono(artifact.revision.clone())));
            if let Some(digest) = &artifact.digest {
                rows.push(("Digest", widgets::kv_mono(digest.clone())));
            }
            if let Some(update) = &artifact.last_update {
                rows.push((
                    "Last fetch",
                    widgets::kv_text(format!("{} ago", widgets::age(Some(update)))),
                ));
            }
            if let Some(size) = artifact.size {
                rows.push(("Size", widgets::kv_mono(format_size(size))));
            }
        }
        let title = if object.kind.is_source() {
            "Source"
        } else {
            object.kind.kind()
        };
        widgets::section(title, colors)
            .child(widgets::kv(rows, colors))
            .into_any_element()
    }

    fn render_inventory(&mut self, cx: &mut Context<Self>) -> AnyElement {
        let colors = cx.colors().clone();
        let Some(object) = self.object.clone() else {
            return widgets::empty("Loading…", &colors);
        };
        let Some(entries) = self.entries() else {
            return widgets::empty(
                match object.kind {
                    FluxKind::HelmRelease => {
                        "No inventory in the status (helm-controller before Flux 2.5): open the Helm release for its objects."
                    }
                    _ => "Nothing applied yet.",
                },
                &colors,
            );
        };
        let nodes = self.inventory_nodes(cx);
        let columns = vec![
            ColumnDef::new(
                "object",
                "Object",
                ColumnWidth::Flex {
                    weight: 1.0,
                    min: 260.0,
                },
            ),
            ColumnDef::new("health", "Health", ColumnWidth::Fixed(150.0)),
            ColumnDef::new(
                "info",
                "Info",
                ColumnWidth::Flex {
                    weight: 0.6,
                    min: 140.0,
                },
            ),
        ];
        let total = entries.len();
        let shown = total.min(self.inventory_shown);
        let rows = nodes.into_iter().enumerate().map(|(index, node)| {
            let open = node.clone();
            let toggle = node.key.clone();
            let chevron: AnyElement = if node.has_children {
                let collapsed = self.collapsed.contains(&node.key);
                div()
                    .id(("flux-node-toggle", index))
                    .cursor_pointer()
                    .on_click(cx.listener(move |this, _, _, cx| {
                        if !this.collapsed.remove(&toggle) {
                            this.collapsed.insert(toggle.clone());
                        }
                        cx.notify();
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
                div().w(u(11.0)).into_any_element()
            };
            let (health, info) = match &node.health {
                Some(h) => (
                    if h.label.is_empty() {
                        div()
                            .text_color(colors.text_dim)
                            .child("present")
                            .into_any_element()
                    } else {
                        widgets::pill(h.label.clone(), tone_color(h.tone, &colors))
                            .into_any_element()
                    },
                    h.info.clone(),
                ),
                None if node.gvr.is_none() => (
                    div()
                        .text_color(colors.text_faint)
                        .child("not served")
                        .into_any_element(),
                    String::new(),
                ),
                None => (
                    div()
                        .text_color(colors.text_faint)
                        .child("…")
                        .into_any_element(),
                    String::new(),
                ),
            };
            let label = match &node.namespace {
                Some(ns) if node.depth == 0 => format!("{ns}/{}", node.name),
                _ => node.name.clone(),
            };
            widgets::row(("flux-node", index), false, 30.0, &colors)
                .cursor_pointer()
                .on_click(
                    cx.listener(move |this, event: &gpui::ClickEvent, window, cx| {
                        if event.click_count() == 2 {
                            this.open_node(&open, window, cx);
                        }
                    }),
                )
                .child(
                    widgets::column_cell(&columns[0]).child(
                        h_flex()
                            .gap(u(6.0))
                            .pl(u(node.depth as f32 * 18.0))
                            .child(chevron)
                            .child(
                                div()
                                    .text_size(u(12.0))
                                    .text_color(colors.text_dim)
                                    .child(node.kind.clone()),
                            )
                            .child(widgets::mono(label))
                            .when(node.depth > 0, |this| this.child(Chip::new("live"))),
                    ),
                )
                .child(widgets::column_cell(&columns[1]).child(health))
                .child(
                    widgets::column_cell(&columns[2]).child(
                        div()
                            .truncate()
                            .text_size(u(12.0))
                            .text_color(colors.text_muted)
                            .child(info),
                    ),
                )
                .into_any_element()
        });
        let summary = inventory::summary(&entries);
        v_flex()
            .flex_1()
            .min_h_0()
            .child(
                h_flex()
                    .flex_none()
                    .h(u(36.0))
                    .px(u(12.0))
                    .gap(u(8.0))
                    .border_b_1()
                    .border_color(colors.border_variant)
                    .text_size(u(12.0))
                    .text_color(colors.text_dim)
                    .child(Icon::new(IconName::ListTree).size(13.0))
                    .child(format!("{total} objects · {summary}"))
                    .child(div().flex_1())
                    .child(Chip::new("live"))
                    .child("children from Kubyl's watches"),
            )
            .child(widgets::header(&columns, &colors))
            .child(
                v_flex()
                    .id("flux-inventory")
                    .flex_1()
                    .min_h_0()
                    .overflow_y_scroll()
                    .children(rows)
                    .when(shown < total, |this| {
                        this.child(
                            div().p(u(10.0)).child(
                                Button::new("flux-inventory-more")
                                    .ghost()
                                    .label(format!(
                                        "Show {} more of {}",
                                        (total - shown).min(INVENTORY_PAGE),
                                        total - shown
                                    ))
                                    .on_click(cx.listener(|this, _, _, cx| {
                                        this.inventory_shown += INVENTORY_PAGE;
                                        this.ensure_inventory(cx);
                                        cx.notify();
                                    })),
                            ),
                        )
                    }),
            )
            .into_any_element()
    }

    fn render_history(&self, cx: &mut Context<Self>) -> AnyElement {
        let colors = cx.colors().clone();
        let Some(object) = self.object.clone() else {
            return widgets::empty("Loading…", &colors);
        };
        if object.kind == FluxKind::HelmRelease {
            let history = helm_history(&object);
            if history.is_empty() {
                return widgets::empty("No history in the status yet.", &colors);
            }
            let columns = vec![
                ColumnDef::new("rev", "Revision", ColumnWidth::Fixed(80.0)),
                ColumnDef::new(
                    "chart",
                    "Chart",
                    ColumnWidth::Flex {
                        weight: 1.0,
                        min: 160.0,
                    },
                ),
                ColumnDef::new("app", "App", ColumnWidth::Fixed(90.0)),
                ColumnDef::new("status", "Status", ColumnWidth::Fixed(130.0)),
                ColumnDef::new("deployed", "Deployed", ColumnWidth::Fixed(90.0)),
                ColumnDef::new("open", "", ColumnWidth::Fixed(130.0)),
            ];
            let storage_ns = helm_storage(&object).map(|(ns, _)| ns);
            let rows = history.iter().enumerate().map(|(index, entry)| {
                let deployed = entry.status == "deployed";
                let cluster = self.cluster().clone();
                let namespace = storage_ns
                    .clone()
                    .unwrap_or_else(|| entry.namespace.clone());
                let secret = entry.storage_secret();
                widgets::row(("flux-hr-history", index), false, 32.0, &colors)
                    .child(
                        widgets::column_cell(&columns[0])
                            .child(widgets::mono(format!("v{}", entry.version))),
                    )
                    .child(
                        widgets::column_cell(&columns[1]).child(widgets::mono(format!(
                            "{} {}",
                            entry.chart_name, entry.chart_version
                        ))),
                    )
                    .child(
                        widgets::column_cell(&columns[2])
                            .child(widgets::mono(entry.app_version.clone().unwrap_or_default())),
                    )
                    .child(widgets::column_cell(&columns[3]).child(widgets::pill(
                        entry.status.clone(),
                        if deployed {
                            colors.green
                        } else if entry.status == "failed" {
                            colors.red
                        } else {
                            colors.text_dim
                        },
                    )))
                    .child(
                        widgets::column_cell(&columns[4])
                            .child(widgets::mono(widgets::age(entry.last_deployed.as_deref()))),
                    )
                    .child(widgets::column_cell(&columns[5]).child(widgets::link(
                        ("flux-hr-rev", index),
                        "Helm release",
                        &colors,
                        move |_, window, cx| {
                            open_helm_release(&cluster, &namespace, &secret, window, cx)
                        },
                    )))
                    .into_any_element()
            });
            return v_flex()
                .flex_1()
                .min_h_0()
                .child(widgets::header(&columns, &colors))
                .child(
                    v_flex()
                        .id("flux-history")
                        .flex_1()
                        .overflow_y_scroll()
                        .children(rows),
                )
                .into_any_element();
        }
        let history = kustomization_history(&object);
        if history.is_empty() {
            return widgets::empty(
                "No history in the status (kustomize-controller before Flux 2.7).",
                &colors,
            );
        }
        let columns = vec![
            ColumnDef::new(
                "revision",
                "Revision",
                ColumnWidth::Flex {
                    weight: 1.0,
                    min: 220.0,
                },
            ),
            ColumnDef::new("status", "Status", ColumnWidth::Fixed(200.0)),
            ColumnDef::new("total", "Runs", ColumnWidth::Fixed(60.0)),
            ColumnDef::new("first", "First", ColumnWidth::Fixed(70.0)),
            ColumnDef::new("last", "Last", ColumnWidth::Fixed(70.0)),
            ColumnDef::new("took", "Took", ColumnWidth::Fixed(90.0)),
        ];
        let rows = history.iter().enumerate().map(|(index, entry)| {
            let ok = entry.status.ends_with("Succeeded");
            widgets::row(("flux-ks-history", index), false, 32.0, &colors)
                .child(
                    widgets::column_cell(&columns[0])
                        .child(widgets::mono(short_revision(&entry.revision))),
                )
                .child(widgets::column_cell(&columns[1]).child(widgets::pill(
                    entry.status.clone(),
                    if ok { colors.green } else { colors.red },
                )))
                .child(
                    widgets::column_cell(&columns[2]).child(widgets::mono(entry.total.to_string())),
                )
                .child(
                    widgets::column_cell(&columns[3])
                        .child(widgets::mono(widgets::age(entry.first.as_deref()))),
                )
                .child(
                    widgets::column_cell(&columns[4])
                        .child(widgets::mono(widgets::age(entry.last.as_deref()))),
                )
                .child(
                    widgets::column_cell(&columns[5])
                        .child(widgets::mono(entry.duration.clone().unwrap_or_default())),
                )
                .into_any_element()
        });
        v_flex()
            .flex_1()
            .min_h_0()
            .child(widgets::header(&columns, &colors))
            .child(
                v_flex()
                    .id("flux-history")
                    .flex_1()
                    .overflow_y_scroll()
                    .children(rows),
            )
            .into_any_element()
    }

    fn render_events(&self, cx: &mut Context<Self>) -> AnyElement {
        let colors = cx.colors().clone();
        let Some(events) = &self.events else {
            return widgets::empty("Loading events…", &colors);
        };
        let rows = self.events(cx);
        if rows.is_empty() {
            return widgets::empty(
                if events.read(cx).status().is_settled() {
                    "No events for this object (Kubernetes keeps them for an hour)."
                } else {
                    "Loading events…"
                },
                &colors,
            );
        }
        let columns = vec![
            ColumnDef::new("type", "Type", ColumnWidth::Fixed(96.0)),
            ColumnDef::new("reason", "Reason", ColumnWidth::Fixed(190.0)),
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
                v_flex()
                    .id("flux-events")
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
                        widgets::row(("flux-event-row", index), false, 30.0, &colors)
                            .child(widgets::column_cell(&columns[0]).child(widgets::pill(
                                if warning { "Warning" } else { "Normal" },
                                if warning {
                                    colors.yellow
                                } else {
                                    colors.text_dim
                                },
                            )))
                            .child(
                                widgets::column_cell(&columns[1]).child(
                                    div()
                                        .truncate()
                                        .text_color(if warning {
                                            colors.yellow
                                        } else {
                                            colors.text
                                        })
                                        .child(
                                            event["reason"]
                                                .as_str()
                                                .unwrap_or_default()
                                                .to_string(),
                                        ),
                                ),
                            )
                            .child(widgets::column_cell(&columns[2]).child(
                                div().truncate().text_size(u(12.0)).child(
                                    kubyl_resources::columns::event_message(event).to_string(),
                                ),
                            ))
                            .child(
                                widgets::column_cell(&columns[3])
                                    .child(widgets::mono(count.to_string())),
                            )
                            .child(widgets::column_cell(&columns[4]).child(widgets::mono(age)))
                    })),
            )
            .into_any_element()
    }

    fn render_logs(&self, cx: &mut Context<Self>) -> AnyElement {
        let colors = cx.colors().clone();
        let install = Flux::try_global(cx).and_then(|f| f.read(cx).install(self.cluster()));
        let controller = install
            .as_ref()
            .and_then(|i| i.controller(self.kind.controller()))
            .map(|c| format!("deployment {}/{}", c.namespace, c.name));
        let missing = match &install {
            Some(install) if install.forbidden => Some(format!(
                "You may not list Deployments in {}: the logs need list on deployments.apps and get on pods/log there.",
                install.namespace.as_deref().unwrap_or("flux-system")
            )),
            Some(_) if controller.is_none() => {
                Some(format!("The {} wasn't found.", self.kind.controller()))
            }
            None => Some("Looking for Flux's controllers…".into()),
            _ => None,
        };
        let query = actions::controller_query(self.kind, self.namespace(), self.name());
        let target = self.target.clone();
        v_flex()
            .p(u(18.0))
            .gap(u(12.0))
            .max_w(u(760.0))
            .child(div().text_size(u(13.0)).child(format!(
                "The {}'s logs, filtered to the lines about this {}.",
                self.kind.controller(),
                self.kind
            )))
            .child(widgets::kv(
                vec![
                    (
                        "Controller",
                        widgets::kv_mono(controller.clone().unwrap_or_else(|| "not found".into())),
                    ),
                    ("Filter", widgets::kv_mono(query)),
                ],
                &colors,
            ))
            .when_some(missing, |this, missing| {
                this.child(
                    widgets::warning_box(&colors)
                        .text_size(u(12.0))
                        .child(
                            Icon::new(IconName::TriangleAlert)
                                .size(13.0)
                                .color(colors.yellow),
                        )
                        .child(missing),
                )
            })
            .child(
                h_flex().child(
                    Button::new("flux-open-logs")
                        .primary()
                        .icon(IconName::Terminal)
                        .label("Open controller logs")
                        .disabled(controller.is_none())
                        .on_click(move |_, window, cx| {
                            actions::controller_logs(&target, window, cx)
                        }),
                ),
            )
            .into_any_element()
    }
}

/// `ConfigMap podinfo-values (key values.yaml)` or `Secret … (values not shown)`.
fn data_ref_row(data: &DataRef, colors: &Colors) -> AnyElement {
    h_flex()
        .gap(u(6.0))
        .text_size(u(12.0))
        .child(
            Icon::new(if data.is_secret() {
                IconName::Lock
            } else {
                IconName::File
            })
            .size(12.0)
            .color(colors.text_dim),
        )
        .child(div().text_color(colors.text_dim).child(data.kind.clone()))
        .child(widgets::mono(data.name.clone()))
        .when_some(data.key.clone(), |this, key| {
            this.child(div().text_color(colors.text_dim).child(format!("· {key}")))
        })
        .when(data.optional, |this| this.child(Chip::new("optional")))
        .when(data.is_secret(), |this| {
            this.child(
                div()
                    .text_color(colors.text_faint)
                    .child("values not shown"),
            )
        })
        .into_any_element()
}

/// Opens the Helm release tab of a revision (phase 12/22: `ViewKind::Custom("helm_release")`
/// with the release's storage Secret as the target).
pub fn open_helm_release(
    cluster: &ClusterId,
    namespace: &str,
    secret: &str,
    window: &mut Window,
    cx: &mut App,
) {
    let target = ResourceRef::object(
        cluster.clone(),
        Gvr::new("", "v1", "secrets"),
        Some(namespace.to_string()),
        secret.to_string(),
    );
    let kind = ViewKind::Custom("helm_release".into());
    if !ViewRegistry::is_registered(cx, &kind) {
        kubyl_core::NotificationCenter::push(
            cx,
            kubyl_core::Notification::error("Helm releases aren't available in this build."),
        );
        return;
    }
    window.dispatch_action(
        Box::new(OpenView(ViewRequest::for_resource(kind, target))),
        cx,
    );
}

fn tone_color(tone: Tone, colors: &Colors) -> gpui::Hsla {
    match tone {
        Tone::Good => colors.green,
        Tone::Bad => colors.red,
        Tone::Warning => colors.yellow,
        Tone::Info => colors.accent,
        _ => colors.text_dim,
    }
}

fn format_size(bytes: i64) -> String {
    match bytes {
        b if b >= 1 << 20 => format!("{:.1} MiB", b as f64 / (1 << 20) as f64),
        b if b >= 1 << 10 => format!("{:.1} KiB", b as f64 / 1024.0),
        b => format!("{b} B"),
    }
}

impl Focusable for ObjectView {
    fn focus_handle(&self, _: &App) -> FocusHandle {
        self.focus.clone()
    }
}

impl TabView for ObjectView {
    fn tab_title(&self, _: &App) -> SharedString {
        self.name().to_string().into()
    }

    fn tab_icon(&self, _: &App) -> Option<SharedString> {
        Some(widgets::kind_icon(self.kind).path())
    }

    fn view_request(&self, _: &App) -> Option<ViewRequest> {
        Some(ViewRequest::for_resource(
            ViewKind::Custom(VIEW_KIND.into()),
            self.target.clone(),
        ))
    }
}

impl Render for ObjectView {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let colors = cx.colors().clone();
        let body = if self.gone {
            widgets::empty(
                format!("{} {} was deleted.", self.kind, self.name()),
                &colors,
            )
        } else if self.object.is_none() {
            widgets::empty(
                match self.own.read(cx).status() {
                    StoreStatus::Forbidden => format!("You may not read this {}.", self.kind),
                    _ => "Loading…".to_string(),
                },
                &colors,
            )
        } else {
            match self.tab {
                Tab::Summary => self.render_summary(cx),
                Tab::Inventory => self.render_inventory(cx),
                Tab::History => self.render_history(cx),
                Tab::Events => self.render_events(cx),
                Tab::Logs => self.render_logs(cx),
            }
        };
        let header = self.render_header(cx);
        let tabs = self.render_tabs(cx);
        let caps = state::cluster_caps(self.cluster(), cx);
        let hints = super::hints(
            OBJECT_CONTEXT,
            Some(&self.target),
            self.object.as_ref(),
            &caps,
            cx,
        );
        v_flex()
            .key_context(OBJECT_CONTEXT)
            .track_focus(&self.focus)
            // Focus listeners only run on the next draw (and not while the window is
            // inactive): a click into the tab makes its object the selection right away.
            .on_mouse_down(
                gpui::MouseButton::Left,
                cx.listener(|this, _, window, cx| {
                    this.focus.focus(window, cx);
                    this.focused = true;
                    this.publish_selection(cx);
                }),
            )
            .size_full()
            .bg(colors.background)
            .text_color(colors.text)
            .font_family(fonts::UI)
            .child(header)
            .child(tabs)
            .child(
                div()
                    .flex_1()
                    .min_h_0()
                    .flex()
                    .flex_col()
                    .child(SelectionScope::new(
                        ("flux-object", cx.entity_id().as_u64()),
                        v_flex().size_full().child(body),
                    )),
            )
            .child(KeyHints::new(hints))
    }
}
