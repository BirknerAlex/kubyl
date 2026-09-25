//! The palette view (board 6 · Palette): a query input with the mode's prefix, mode chips,
//! grouped results with match highlights, and a footer with the keys.

use std::collections::{HashMap, HashSet};
use std::time::Duration;

use gpui::{
    App, AppContext as _, Context, Entity, FocusHandle, Focusable, FontWeight, HighlightStyle,
    IntoElement, KeyContext, Render, ScrollHandle, SharedString, StyledText, Subscription, Task,
    Window, div, prelude::*,
};
use gpui_component::WindowExt as _;
use gpui_component::input::{Input, InputEvent, InputState};
use kubyl_core::actions::OpenView;
use kubyl_core::{
    ActionRegistry, ActiveContext, ClusterId, Gvk, Notification, NotificationCenter, ResourceRef,
    ViewRegistry, ViewRequest,
};
use kubyl_explorer::favorites::{self, Favorites};
use kubyl_kube::ConnectionManager;
use kubyl_resources::{ResourceSelection, ResourceStore, ResourceStores, StoreHandle, StoreKey};
use kubyl_ui::{ActiveColors, Icon, IconName, Kbd, fonts, h_flex, u, v_flex};

use crate::command::{Mode, Scope};
use crate::items::{
    self, ActionEntry, ContextEntry, FavoriteEntry, Item, KindEntry, ObjectEntry, Options,
    ResolvedRef, Snapshot, Target, Trailing,
};
use crate::matcher::byte_ranges;
use crate::recent::Recent;
use crate::references::{RefTarget, references};
use crate::{
    ClearMode, Confirm, ConfirmInSplit, Dismiss, SelectNext, SelectPrevious, ToggleAllNamespaces,
    keys_for,
};

pub(crate) const CONTEXT: &str = "CommandPalette";
/// Added to the key context while the query is empty in a prefixed mode: backspace leaves it.
pub(crate) const EMPTY_CONTEXT: &str = "CommandPaletteEmpty";
/// Kinds whose live count is shown (the first results).
const COUNTED: usize = 8;
const COUNT_DEBOUNCE: Duration = Duration::from_millis(150);

/// Where the palette was opened: the focus to return to and its key contexts (which decide the
/// actions that apply and the keys shown for them).
pub(crate) struct Origin {
    pub focus: Option<FocusHandle>,
    pub contexts: Vec<KeyContext>,
}

impl Origin {
    /// `Pods` when a resource list had focus.
    fn list_label(&self) -> Option<String> {
        let list = self.contexts.iter().find(|c| c.contains("ResourceList"))?;
        Some(match list.get("kind") {
            Some(kind) => format!("{kind}s"),
            None => "list".into(),
        })
    }
}

/// A store a count is read from: held (a metadata watch started for the palette) or borrowed
/// (a list already watches the same thing).
enum Counted {
    Held(StoreHandle),
    Borrowed(Entity<ResourceStore>),
}

impl Counted {
    fn entity(&self) -> &Entity<ResourceStore> {
        match self {
            Counted::Held(handle) => handle.entity(),
            Counted::Borrowed(entity) => entity,
        }
    }
}

pub struct CommandPalette {
    query: Entity<InputState>,
    mode: Mode,
    options: Options,
    items: Vec<Item>,
    selected: usize,
    origin: Origin,
    /// Loaded objects, gathered once (Objects and the mixed mode).
    objects: Option<std::sync::Arc<Vec<ObjectEntry>>>,
    counts: HashMap<StoreKey, (Counted, Subscription)>,
    count_task: Option<Task<()>>,
    scroll: ScrollHandle,
    focus: FocusHandle,
    _subscriptions: Vec<Subscription>,
}

impl CommandPalette {
    pub(crate) fn new(
        mode: Mode,
        query: &str,
        origin: Origin,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Self {
        let (mode, text) = match Mode::split(query) {
            Some((prefixed, rest)) if mode == Mode::All => (prefixed, rest.to_string()),
            _ => (mode, query.to_string()),
        };
        let input = cx.new(|cx| {
            let mut input = InputState::new(window, cx).placeholder(mode.placeholder());
            input.set_value(text, window, cx);
            input
        });
        let mut subscriptions =
            vec![
                cx.subscribe_in(&input, window, |this, _, event: &InputEvent, window, cx| {
                    match event {
                        InputEvent::Change => this.query_changed(window, cx),
                        InputEvent::PressEnter { secondary, .. } => {
                            this.confirm(*secondary, window, cx)
                        }
                        _ => {}
                    }
                }),
            ];
        if let Some(manager) = ConnectionManager::try_global(cx) {
            subscriptions.push(cx.observe_in(&manager, window, |this, _, window, cx| {
                this.refresh(window, cx)
            }));
        }
        subscriptions.push(
            cx.observe_in(&Favorites::global(cx), window, |this, _, window, cx| {
                this.refresh(window, cx)
            }),
        );
        let mut this = Self {
            query: input,
            mode,
            options: Options::default(),
            items: Vec::new(),
            selected: 0,
            origin,
            objects: None,
            counts: HashMap::new(),
            count_task: None,
            scroll: ScrollHandle::new(),
            focus: cx.focus_handle(),
            _subscriptions: subscriptions,
        };
        this.refresh(window, cx);
        this
    }

    pub(crate) fn query_focus(&self, cx: &App) -> FocusHandle {
        self.query.read(cx).focus_handle(cx)
    }

    fn text(&self, cx: &App) -> String {
        self.query.read(cx).value().to_string()
    }

    fn query_changed(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let text = self.text(cx);
        // Typing a prefix switches the mode (`/` mode keeps it: `/regex/` filters).
        if let Some((mode, rest)) = Mode::split(&text)
            && (self.mode == Mode::All || (rest.is_empty() && self.mode != Mode::Filter))
        {
            let rest = rest.to_string();
            self.set_mode(mode, Some(rest), window, cx);
            return;
        }
        self.selected = 0;
        self.refresh(window, cx);
    }

    /// Switches the mode; `text` replaces the query (`None` keeps it).
    fn set_mode(
        &mut self,
        mode: Mode,
        text: Option<String>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.mode = mode;
        self.query.update(cx, |input, cx| {
            if let Some(text) = text {
                input.set_value(text, window, cx);
            }
            input.set_placeholder(mode.placeholder(), window, cx);
        });
        self.selected = 0;
        self.refresh(window, cx);
    }

    pub(crate) fn refresh(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if matches!(self.mode, Mode::All | Mode::Objects) && self.objects.is_none() {
            self.objects = Some(std::sync::Arc::new(gather_objects(cx)));
        }
        let snapshot = self.snapshot(cx);
        self.items = items::build(self.mode, &self.text(cx), &snapshot, self.options);
        self.selected = self.selected.min(self.items.len().saturating_sub(1));
        self.schedule_counts(window, cx);
        cx.notify();
    }

    fn snapshot(&self, cx: &App) -> Snapshot {
        let mut snapshot = Snapshot {
            recent: Recent::load(cx),
            list: self.origin.list_label(),
            objects: self.objects.clone().unwrap_or_default(),
            actions: self.actions(cx),
            favorites: gather_favorites(cx),
            ..Default::default()
        };
        let active = ActiveContext::global(cx);
        snapshot.namespace = active.namespace.as_ref().map(|n| n.to_string());
        let Some(manager) = ConnectionManager::try_global(cx) else {
            return snapshot;
        };
        let manager = manager.read(cx);
        snapshot.contexts = manager
            .contexts()
            .map(|c| {
                let state = manager.state(&c.id);
                ContextEntry {
                    id: c.id.clone(),
                    name: manager.display_name(&c.id).to_string(),
                    context: c.context.clone(),
                    server: c.server.clone(),
                    state: state.label(),
                    color: manager.color(&c.id, cx),
                    connected: state.is_connected(),
                    production: manager.caps(&c.id).production,
                }
            })
            .collect();
        if let Some(cluster) = active.cluster.as_ref() {
            snapshot.cluster = Some((cluster.id.clone(), cluster.name.to_string()));
            let namespaces = manager.namespaces(&cluster.id);
            snapshot.namespaces = namespaces.names;
            snapshot.namespaces_listed = namespaces.listed;
            if let Some(discovery) = manager.discovery(&cluster.id) {
                snapshot.kinds = discovery
                    .preferred()
                    .filter(|r| r.is_listable())
                    .map(|r| KindEntry {
                        gvr: r.gvr.clone(),
                        kind: r.gvk.kind.clone(),
                        singular: r.singular.clone(),
                        short_names: r.short_names.clone(),
                        categories: r.categories.clone(),
                        namespaced: r.namespaced,
                        icon: kubyl_explorer::catalog::icon_for(&r.gvr.group, &r.gvr.resource),
                    })
                    .collect();
            }
        }
        if self.mode == Mode::References {
            snapshot.references = selection_references(cx);
        }
        snapshot
    }

    /// Registered actions that apply where the palette was opened, with the keys that run them
    /// there.
    fn actions(&self, cx: &App) -> Vec<ActionEntry> {
        let selection = ResourceSelection::global(cx);
        let target = selection.primary().map(|s| s.target.clone());
        let manager = ConnectionManager::try_global(cx);
        let keymap = cx.key_bindings();
        let keymap = keymap.borrow();
        ActionRegistry::global(cx)
            .all()
            .iter()
            .enumerate()
            .filter(|(_, spec)| {
                let Some(context) = spec.context.as_deref() else {
                    return true;
                };
                gpui::KeyBindingContextPredicate::parse(context)
                    .is_ok_and(|p| p.eval(&self.origin.contexts))
            })
            .filter(|(_, spec)| {
                if spec.available.is_none() {
                    return true;
                }
                let Some(target) = &target else {
                    return false;
                };
                if !spec.is_available(target, &selection.caps) {
                    return false;
                }
                // RBAC: hide what the user may not do (unknown = shown, like the hint bar).
                match (
                    kubyl_explorer::actions::access_for(&spec.name, target),
                    &manager,
                ) {
                    (Some(query), Some(manager)) => manager
                        .read(cx)
                        .cached_can_i(&target.cluster, &query)
                        .unwrap_or(true),
                    _ => true,
                }
            })
            .map(|(index, spec)| ActionEntry {
                index,
                depth: spec
                    .context
                    .as_deref()
                    .and_then(|c| gpui::KeyBindingContextPredicate::parse(c).ok())
                    .and_then(|p| p.depth_of(&self.origin.contexts))
                    .unwrap_or(0) as u32,
                name: spec.name.to_string(),
                keys: keys_for(&keymap, &*spec.action(), &self.origin.contexts)
                    .or_else(|| spec.keystrokes.as_ref().map(|k| k.to_string())),
            })
            .collect()
    }

    // ----- Live counts -----

    fn schedule_counts(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.count_task = Some(cx.spawn_in(window, async move |this, cx| {
            cx.background_executor().timer(COUNT_DEBOUNCE).await;
            this.update(cx, |this, cx| this.sync_counts(cx)).ok();
        }));
    }

    /// Counts the first kinds: borrows a list's store when one exists, else starts a metadata
    /// watch (released when the palette closes).
    fn sync_counts(&mut self, cx: &mut Context<Self>) {
        let wanted: Vec<StoreKey> = self
            .items
            .iter()
            .filter_map(|i| match &i.trailing {
                Trailing::Count(key) => Some(key.clone()),
                _ => None,
            })
            .take(COUNTED)
            .collect();
        let wanted_set: HashSet<&StoreKey> = wanted.iter().collect();
        self.counts.retain(|key, _| wanted_set.contains(key));
        let manager = ConnectionManager::try_global(cx);
        for key in wanted {
            if self.counts.contains_key(&key) {
                continue;
            }
            let connected = manager
                .as_ref()
                .is_some_and(|m| m.read(cx).state(&key.cluster).is_connected());
            if !connected {
                continue;
            }
            let full = StoreKey {
                mode: kubyl_resources::StoreMode::Full,
                ..key.clone()
            };
            let counted =
                match ResourceStores::peek(cx, &full).or_else(|| ResourceStores::peek(cx, &key)) {
                    Some(entity) => Counted::Borrowed(entity),
                    None => Counted::Held(ResourceStores::acquire(cx, key.clone())),
                };
            let subscription = cx.observe(counted.entity(), |_, _, cx| cx.notify());
            self.counts.insert(key, (counted, subscription));
        }
        cx.notify();
    }

    fn count(&self, key: &StoreKey, cx: &App) -> Option<usize> {
        let (counted, _) = self.counts.get(key)?;
        let store = counted.entity().read(cx);
        store.status().is_ready().then(|| store.len())
    }

    // ----- Keys -----

    fn move_selection(&mut self, delta: isize, cx: &mut Context<Self>) {
        if self.items.is_empty() {
            return;
        }
        let len = self.items.len() as isize;
        self.selected = (self.selected as isize + delta).rem_euclid(len) as usize;
        self.scroll_to_selected();
        cx.notify();
    }

    fn scroll_to_selected(&self) {
        // Rows include one header per group.
        let mut row = 0;
        let mut group = None;
        for (ix, item) in self.items.iter().enumerate() {
            if group != Some(item.group) {
                group = Some(item.group);
                row += 1;
            }
            if ix == self.selected {
                break;
            }
            row += 1;
        }
        self.scroll.scroll_to_item(row);
    }

    fn toggle_all_namespaces(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.options.all_namespaces = !self.options.all_namespaces;
        self.refresh(window, cx);
    }

    fn clear_mode(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.set_mode(Mode::All, None, window, cx);
    }

    fn dismiss(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        window.close_dialog(cx);
    }

    // ----- Confirm -----

    fn confirm(&mut self, split: bool, window: &mut Window, cx: &mut Context<Self>) {
        let Some(item) = self.items.get(self.selected).cloned() else {
            return;
        };
        tracing::debug!(title = %item.title, target = ?item.target, split, "palette confirm");
        if let Target::Mode(mode) = item.target {
            self.set_mode(mode, Some(String::new()), window, cx);
            return;
        }
        if let Some(key) = &item.key {
            Recent::touch(cx, key);
        }
        // Closing the dialog returns focus to where the palette was opened, so actions dispatch
        // there.
        window.close_dialog(cx);
        if let Some(focus) = &self.origin.focus {
            window.focus(focus, cx);
        }
        run(item.target, split, window, cx);
    }
}

/// Runs a confirmed result.
fn run(target: Target, split: bool, window: &mut Window, cx: &mut App) {
    let opens_view = matches!(
        target,
        Target::Kind { .. }
            | Target::Object(_)
            | Target::FavoritesWorkspace(_)
            | Target::Filtered { .. }
            | Target::Favorite(_)
    );
    if split && opens_view {
        match cx.build_action("pane::SplitRight", None) {
            Ok(action) => {
                window.dispatch_action(action, cx);
                // The pane asks the workspace to split through an event, which is handled after
                // actions already queued: open the view one turn later, in the new pane.
                window.defer(cx, move |window, cx| run(target, false, window, cx));
                return;
            }
            Err(err) => tracing::warn!("split: {err}"),
        }
    }
    let open = |window: &mut Window, cx: &mut App, request: ViewRequest| {
        window.dispatch_action(Box::new(OpenView(request)), cx)
    };
    match target {
        Target::Kind {
            cluster,
            gvr,
            namespaced,
            scope,
        } => {
            let namespace = match (&scope, namespaced) {
                (Scope::Named(ns), true) => Some(ns.clone()),
                _ => None,
            };
            // `-A` / `⇥` and `:pods payments` switch the namespace, like k9s.
            let active = ActiveContext::global(cx).clone();
            if namespaced
                && active.cluster.as_ref().map(|c| &c.id) == Some(&cluster)
                && scope != Scope::Active
            {
                ActiveContext::set(
                    cx,
                    ActiveContext {
                        namespace: namespace.clone().map(SharedString::from),
                        ..active
                    },
                );
            }
            // A kind's own view when a crate registered one (`:apps` → Argo CD Applications).
            let kind = ViewRegistry::list_view(cx, &gvr);
            open(
                window,
                cx,
                ViewRequest::for_resource(kind, ResourceRef::list(cluster, gvr, namespace)),
            );
        }
        Target::Context(id) => {
            if let Some(manager) = ConnectionManager::try_global(cx) {
                manager.update(cx, |m, cx| m.activate(&id, cx));
            }
        }
        Target::Namespace(namespace) => {
            let active = ActiveContext::global(cx).clone();
            ActiveContext::set(
                cx,
                ActiveContext {
                    namespace: namespace.map(SharedString::from),
                    ..active
                },
            );
        }
        Target::Action(index) => {
            let action = ActionRegistry::global(cx)
                .all()
                .get(index)
                .map(|spec| spec.action());
            if let Some(action) = action {
                window.dispatch_action(action, cx);
            }
        }
        Target::Favorite(index) => {
            let favorite = Favorites::global(cx).read(cx).items().get(index).cloned();
            if let Some(favorite) = favorite {
                kubyl_explorer::sidebar::open_favorite(&favorite, window, cx);
            }
        }
        Target::AddFavorite { cluster, namespace } => {
            kubyl_explorer::actions::add_favorite(&cluster, &namespace, cx)
        }
        Target::FavoritesWorkspace(gvr) => open(
            window,
            cx,
            ViewRequest::for_resource(
                kubyl_explorer::favorites_view_kind(),
                ResourceRef::list(ClusterId::new(""), gvr, None),
            ),
        ),
        Target::Object(target) => {
            let kind = ViewRegistry::object_view(cx, &target.gvr);
            open(window, cx, ViewRequest::for_resource(kind, target))
        }
        Target::Filtered {
            cluster,
            gvr,
            namespace,
            filter,
        } => kubyl_explorer::list::open_filtered(cluster, gvr, namespace, filter, window, cx),
        Target::Filter(query) => {
            window.dispatch_action(Box::new(kubyl_explorer::list::SetFilter { query }), cx)
        }
        Target::Quit => match cx.build_action("kubyl_app::Quit", None) {
            Ok(action) => window.dispatch_action(action, cx),
            Err(_) => cx.quit(),
        },
        Target::Notice(message) => NotificationCenter::push(cx, Notification::info(message)),
        Target::Mode(_) => {}
    }
}

fn gather_favorites(cx: &App) -> Vec<FavoriteEntry> {
    let manager = ConnectionManager::try_global(cx);
    Favorites::global(cx)
        .read(cx)
        .items()
        .iter()
        .enumerate()
        .map(|(index, fav)| {
            let cluster = favorites::cluster_of(fav, cx);
            let cluster_name = match (&cluster, &manager) {
                (Some(id), Some(m)) => m.read(cx).display_name(id).to_string(),
                _ => fav.context.clone(),
            };
            FavoriteEntry {
                index,
                cluster_id: cluster.clone(),
                namespace: fav.namespace.clone(),
                label: fav.label().to_string(),
                cluster: cluster_name,
                kind: fav.kind.clone().unwrap_or_else(|| "pods".into()),
                selector: fav.selector.clone(),
                resolved: cluster.is_some(),
            }
        })
        .collect()
}

/// Every object in the loaded caches of connected clusters, once per object.
fn gather_objects(cx: &App) -> Vec<ObjectEntry> {
    let Some(manager) = ConnectionManager::try_global(cx) else {
        return Vec::new();
    };
    let manager = manager.read(cx);
    let mut seen = HashSet::new();
    let mut objects = Vec::new();
    for store in ResourceStores::all(cx) {
        let store = store.read(cx);
        let key = store.key();
        if !manager.state(&key.cluster).is_connected() {
            continue;
        }
        let cluster_name = manager.display_name(&key.cluster).to_string();
        let kind_name = manager
            .discovery(&key.cluster)
            .and_then(|d| d.by_gvr(&key.gvr).map(|r| r.gvk.kind.clone()));
        let icon = kubyl_explorer::catalog::icon_for(&key.gvr.group, &key.gvr.resource);
        for object in store.objects().values() {
            let Some(name) = object.pointer("/metadata/name").and_then(|n| n.as_str()) else {
                continue;
            };
            let namespace = object
                .pointer("/metadata/namespace")
                .and_then(|n| n.as_str())
                .map(String::from);
            let identity = (
                key.cluster.clone(),
                key.gvr.group.clone(),
                key.gvr.resource.clone(),
                namespace.clone(),
                name.to_string(),
            );
            if !seen.insert(identity) {
                continue;
            }
            // Metadata-only stores say `PartialObjectMetadata`; discovery knows the kind.
            let kind = kind_name
                .clone()
                .or_else(|| {
                    object
                        .pointer("/kind")
                        .and_then(|k| k.as_str())
                        .map(String::from)
                })
                .unwrap_or_else(|| key.gvr.resource.clone());
            objects.push(ObjectEntry {
                target: ResourceRef::object(
                    key.cluster.clone(),
                    key.gvr.clone(),
                    namespace,
                    name.to_string(),
                ),
                kind,
                cluster_name: cluster_name.clone(),
                icon,
            });
        }
    }
    objects.sort_by(|a, b| a.target.name.cmp(&b.target.name));
    objects
}

/// References of the selected object, resolved against its cluster's discovery.
fn selection_references(cx: &App) -> Vec<(crate::references::Reference, Option<ResolvedRef>)> {
    let selection = ResourceSelection::global(cx);
    let Some(selected) = selection.primary() else {
        return Vec::new();
    };
    // Prefer the live object from its store (the selection may hold an older copy).
    let object = selected
        .store
        .as_ref()
        .and_then(|store| {
            let key = kubyl_resources::object_key(
                selected.target.namespace.as_deref(),
                selected.target.name.as_deref().unwrap_or_default(),
            );
            store.read(cx).get(&key).cloned()
        })
        .or_else(|| selected.object.clone());
    let Some(object) = object else {
        return Vec::new();
    };
    let cluster = selected.target.cluster.clone();
    let discovery = ConnectionManager::try_global(cx).and_then(|m| m.read(cx).discovery(&cluster));
    references(&selected.kind, &object)
        .into_iter()
        .map(|reference| {
            let gvk: &Gvk = match &reference.target {
                RefTarget::Object { gvk, .. } | RefTarget::Filtered { gvk, .. } => gvk,
            };
            let resolved = discovery.as_ref().and_then(|d| {
                let info = d.by_gvk(gvk).or_else(|| {
                    d.preferred()
                        .find(|r| r.gvk.kind == gvk.kind && r.gvk.group == gvk.group)
                })?;
                Some(ResolvedRef {
                    cluster: cluster.clone(),
                    gvr: info.gvr.clone(),
                    namespaced: info.namespaced,
                    icon: kubyl_explorer::catalog::icon_for(&info.gvr.group, &info.gvr.resource),
                })
            });
            (reference, resolved)
        })
        .collect()
}

impl Focusable for CommandPalette {
    fn focus_handle(&self, _: &App) -> FocusHandle {
        self.focus.clone()
    }
}

impl CommandPalette {
    fn key_context(&self, cx: &App) -> KeyContext {
        let mut context = KeyContext::new_with_defaults();
        context.add(CONTEXT);
        if self.mode != Mode::All && self.query.read(cx).value().is_empty() {
            context.add(EMPTY_CONTEXT);
        }
        context
    }

    fn render_header(&self, cx: &mut Context<Self>) -> impl IntoElement {
        let colors = cx.colors().clone();
        let active = ActiveContext::global(cx);
        let mut scope = match &active.cluster {
            Some(cluster) => format!("in {}", cluster.name),
            None => "no cluster".into(),
        };
        if matches!(self.mode, Mode::All | Mode::Resources) {
            if self.options.all_namespaces {
                scope.push_str(" · all namespaces");
            } else if let Some(ns) = &active.namespace {
                scope.push_str(&format!(" · {ns}"));
            }
        }
        let prefix = match self.mode {
            Mode::All => Icon::new(IconName::Search)
                .size(14.0)
                .color(colors.text_dim)
                .into_any_element(),
            Mode::Objects => Icon::new(IconName::Box)
                .size(14.0)
                .color(colors.accent)
                .into_any_element(),
            Mode::References => Icon::new(IconName::Link)
                .size(14.0)
                .color(colors.accent)
                .into_any_element(),
            mode => div()
                .font_family(fonts::MONO)
                .text_size(u(14.0))
                .text_color(colors.accent)
                .child(mode.prefix().unwrap_or(' ').to_string())
                .into_any_element(),
        };
        h_flex()
            .h(u(44.0))
            .px(u(14.0))
            .gap(u(10.0))
            .border_b_1()
            .border_color(colors.border_variant)
            .child(prefix)
            .child(
                div().flex_1().min_w_0().child(
                    Input::new(&self.query)
                        .appearance(false)
                        .font_family(fonts::MONO)
                        .text_size(u(14.0)),
                ),
            )
            .child(
                div()
                    .flex_none()
                    .text_size(u(11.5))
                    .text_color(colors.text_dim)
                    .child(scope),
            )
    }

    fn render_chips(&self, cx: &mut Context<Self>) -> impl IntoElement {
        let colors = cx.colors().clone();
        let has_list = self.origin.list_label().is_some();
        let chips = Mode::PREFIXED
            .into_iter()
            .filter(|m| *m != Mode::Filter || has_list)
            .map(|mode| {
                let on = self.mode == mode;
                let (bg, fg) = if on {
                    (colors.chip_selected_background, colors.chip_selected_text)
                } else {
                    (colors.chip_background, colors.text_muted)
                };
                h_flex()
                    .id(SharedString::from(format!("mode-{}", mode.label())))
                    .h(u(20.0))
                    .px(u(7.0))
                    .gap(u(4.0))
                    .rounded(u(4.0))
                    .bg(bg)
                    .text_color(fg)
                    .cursor_pointer()
                    .when(on, |this| {
                        this.border_1().border_color(colors.chip_selected_border)
                    })
                    .child(
                        div()
                            .font_family(fonts::MONO)
                            .font_weight(FontWeight::BOLD)
                            .child(mode.prefix().unwrap_or(' ').to_string()),
                    )
                    .child(mode.label())
                    .on_click(cx.listener(move |this, _, window, cx| {
                        let mode = if this.mode == mode { Mode::All } else { mode };
                        this.set_mode(mode, None, window, cx);
                        let focus = this.query_focus(cx);
                        window.focus(&focus, cx);
                    }))
            });
        h_flex()
            .gap(u(6.0))
            .px(u(12.0))
            .py(u(8.0))
            .border_b_1()
            .border_color(colors.border_variant)
            .text_size(u(11.5))
            .children(chips)
    }

    fn render_results(&self, cx: &mut Context<Self>) -> impl IntoElement {
        let colors = cx.colors().clone();
        let mut rows = Vec::new();
        let mut group = None;
        for (ix, item) in self.items.iter().enumerate() {
            if group != Some(item.group) {
                group = Some(item.group);
                rows.push(
                    div()
                        .pt(u(8.0))
                        .pb(u(4.0))
                        .px(u(12.0))
                        .text_size(u(11.0))
                        .font_weight(FontWeight::SEMIBOLD)
                        .text_color(colors.text_dim)
                        .child(item.group.label().to_uppercase())
                        .into_any_element(),
                );
            }
            rows.push(self.render_item(ix, item, cx).into_any_element());
        }
        if self.items.is_empty() {
            rows.push(
                div()
                    .px(u(12.0))
                    .py(u(10.0))
                    .text_color(colors.text_dim)
                    .child(self.empty_message(cx))
                    .into_any_element(),
            );
        }
        div()
            .id("palette-results")
            .max_h(u(440.0))
            .overflow_y_scroll()
            .track_scroll(&self.scroll)
            .child(v_flex().pt(u(4.0)).px(u(6.0)).pb(u(8.0)).children(rows))
    }

    fn empty_message(&self, cx: &App) -> String {
        let text = self.text(cx);
        match self.mode {
            Mode::Objects if text.trim().is_empty() => {
                "Type a name. Searches what open lists and the sidebar have loaded.".into()
            }
            Mode::Resources | Mode::Namespaces if ActiveContext::global(cx).cluster.is_none() => {
                "Pick a cluster first (@ switches contexts).".into()
            }
            Mode::References => "The selection refers to nothing Kubyl knows about.".into(),
            _ => "No matches.".into(),
        }
    }

    fn render_item(&self, ix: usize, item: &Item, cx: &mut Context<Self>) -> impl IntoElement {
        let colors = cx.colors().clone();
        let selected = ix == self.selected;
        let hover = colors.hover;
        let highlight = HighlightStyle {
            color: Some(colors.accent),
            font_weight: Some(FontWeight::SEMIBOLD),
            ..Default::default()
        };
        let title = StyledText::new(item.title.clone()).with_highlights(
            byte_ranges(&item.title, &item.positions)
                .into_iter()
                .map(|range| (range, highlight)),
        );
        let icon_color = if selected || item.current {
            colors.accent
        } else {
            colors.text_dim
        };
        let trailing = match &item.trailing {
            Trailing::None => None,
            Trailing::Count(key) => Some(
                div()
                    .w(u(34.0))
                    .flex_none()
                    .text_right()
                    .font_family(fonts::MONO)
                    .text_size(u(11.5))
                    .text_color(colors.text_muted)
                    .children(self.count(key, cx).map(|n| n.to_string()))
                    .into_any_element(),
            ),
            Trailing::Keys(keys) => Some(Kbd::keystroke(keys).into_any_element()),
            Trailing::Text(text) => Some(
                div()
                    .flex_none()
                    .text_size(u(11.5))
                    .text_color(colors.text_dim)
                    .child(text.clone())
                    .into_any_element(),
            ),
        };
        h_flex()
            .id(("palette-item", ix))
            .h(u(32.0))
            .px(u(12.0))
            .gap(u(10.0))
            .rounded(u(5.0))
            .cursor_pointer()
            .when(selected, |this| this.bg(colors.selection))
            .when(!selected, |this| this.hover(move |s| s.bg(hover)))
            .on_click(
                cx.listener(move |this, event: &gpui::ClickEvent, window, cx| {
                    this.selected = ix;
                    this.confirm(event.modifiers().secondary(), window, cx);
                }),
            )
            .child(Icon::new(item.icon).size(14.0).color(icon_color))
            .when_some(item.color, |this, color| {
                this.child(kubyl_ui::StatusDot::new(color))
            })
            .child(
                div()
                    .flex_none()
                    .max_w(u(300.0))
                    .truncate()
                    .text_color(if item.current {
                        colors.accent
                    } else {
                        colors.text
                    })
                    .child(title),
            )
            .children(item.detail.clone().map(|detail| {
                div()
                    .min_w_0()
                    .truncate()
                    .font_family(fonts::MONO)
                    .text_size(u(11.5))
                    .text_color(colors.text_dim)
                    .child(detail)
            }))
            .child(div().flex_1())
            .children(item.aliases.clone().map(|aliases| {
                div()
                    .flex_none()
                    .font_family(fonts::MONO)
                    .text_size(u(11.0))
                    .text_color(colors.text_dim)
                    .child(aliases)
            }))
            .children(trailing)
    }

    fn render_footer(&self, cx: &mut Context<Self>) -> impl IntoElement {
        let colors = cx.colors().clone();
        let hint = |keys: &str, label: &'static str| {
            h_flex()
                .gap(u(5.0))
                .child(Kbd::keystroke(keys))
                .when(!label.is_empty(), |this| this.child(label))
        };
        let (open, split) = match self.items.get(self.selected).map(|i| &i.target) {
            Some(Target::Context(_) | Target::Namespace(_)) => ("switch", false),
            Some(Target::Action(_)) => ("run", false),
            Some(Target::Filter(_)) => ("filter", false),
            Some(Target::Mode(_)) => ("list", false),
            _ => ("open", true),
        };
        h_flex()
            .gap(u(16.0))
            .px(u(14.0))
            .py(u(8.0))
            .border_t_1()
            .border_color(colors.border_variant)
            .text_size(u(11.5))
            .text_color(colors.text_dim)
            .child(hint("enter", open))
            .when(split, |this| {
                this.child(hint("secondary-enter", "open in split"))
            })
            .when(matches!(self.mode, Mode::All | Mode::Resources), |this| {
                this.child(hint(
                    "tab",
                    if self.options.all_namespaces {
                        "active namespace"
                    } else {
                        "all namespaces"
                    },
                ))
            })
            .child(div().flex_1())
            .child(hint("escape", ""))
    }
}

impl Render for CommandPalette {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let colors = cx.colors().clone();
        v_flex()
            .key_context(self.key_context(cx))
            .track_focus(&self.focus)
            .on_action(cx.listener(|this, _: &SelectNext, _, cx| this.move_selection(1, cx)))
            .on_action(cx.listener(|this, _: &SelectPrevious, _, cx| this.move_selection(-1, cx)))
            .on_action(cx.listener(|this, _: &ToggleAllNamespaces, window, cx| {
                this.toggle_all_namespaces(window, cx)
            }))
            .on_action(cx.listener(|this, _: &ClearMode, window, cx| this.clear_mode(window, cx)))
            .on_action(cx.listener(|this, _: &Dismiss, window, cx| this.dismiss(window, cx)))
            .on_action(cx.listener(|this, _: &Confirm, window, cx| this.confirm(false, window, cx)))
            .on_action(
                cx.listener(|this, _: &ConfirmInSplit, window, cx| this.confirm(true, window, cx)),
            )
            .text_color(colors.text)
            .text_size(u(13.0))
            .child(self.render_header(cx))
            .child(self.render_chips(cx))
            .child(self.render_results(cx))
            .child(self.render_footer(cx))
    }
}
