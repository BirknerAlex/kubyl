//! A Flux list (board 21): Kustomizations, HelmReleases, Sources, Image Automation or
//! Notifications of a cluster, with name, namespace, state and message, source, revision,
//! suspended, interval, last reconcile and age; filters by state, namespace and kind; counts in
//! the toolbar. Several rows can be selected (⌘/ctrl-click, shift-click) to reconcile, suspend
//! or resume them together; the details dock follows the selection.

use std::collections::BTreeSet;
use std::ops::Range;
use std::sync::Arc;

use gpui::{
    App, AppContext as _, ClickEvent, Context, Entity, FocusHandle, Focusable, FontWeight,
    IntoElement, Render, ScrollStrategy, SharedString, Subscription, UniformListScrollHandle,
    Window, div, prelude::*, uniform_list,
};
use gpui_component::button::{Button as MenuButton, ButtonVariants as _};
use gpui_component::input::{Input, InputEvent, InputState};
use gpui_component::menu::{DropdownMenu as _, PopupMenuItem};
use kubyl_core::{
    ActiveContext, ClusterId, ColumnDef, ColumnWidth, Gvr, ResourceRef, TabView, ViewKind,
    ViewRequest,
};
use kubyl_flux_core::kinds::{Category, FluxKind};
use kubyl_flux_core::model::State;
use kubyl_flux_core::rows::{self, Filters, Row};
use kubyl_kube::{ConnectionEvent, ConnectionManager};
use kubyl_resources::{ResourceSelection, ResourceStores, Selected, StoreHandle, StoreStatus};
use kubyl_ui::{
    ActiveColors, Chip, Colors, Icon, IconName, KeyHints, fonts, h_flex, sizes, u, v_flex,
};

use crate::actions::{self, LIST_CONTEXT};
use crate::nav::{self, Move, TABLE};
use crate::state;
use crate::widgets;

const ROW_HEIGHT: f32 = 32.0;

pub struct ListView {
    cluster: ClusterId,
    category: Category,
    /// Kinds of the category the cluster serves, with their watches.
    stores: Vec<(FluxKind, Gvr, StoreHandle)>,
    all: Vec<Row>,
    rows: Vec<Row>,
    filters: Filters,
    sort: (SharedString, bool),
    /// Selected row keys; the first is the primary one.
    selected: Vec<Arc<str>>,
    /// Where shift-click ranges start.
    anchor: Option<Arc<str>>,
    filter_input: Entity<InputState>,
    focus: FocusHandle,
    /// The table has focus: only then does the list publish its selection (a list in a
    /// background tab must not take the global selection over on every store update).
    focused: bool,
    scroll: UniformListScrollHandle,
    parsed: state::ParsedObjects,
    _store_observers: Vec<Subscription>,
    _subscriptions: Vec<Subscription>,
}

impl ListView {
    pub fn new(
        category: Category,
        target: ResourceRef,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Self {
        let filter_input = cx.new(|cx| InputState::new(window, cx).placeholder("Filter"));
        let mut subscriptions = vec![cx.subscribe_in(
            &filter_input,
            window,
            |this, input, event: &InputEvent, window, cx| match event {
                InputEvent::Change => {
                    this.filters.text = input.read(cx).value().to_string();
                    this.refresh(cx);
                }
                InputEvent::PressEnter { .. } => this.focus.focus(window, cx),
                _ => {}
            },
        )];
        if let Some(manager) = ConnectionManager::try_global(cx) {
            let cluster = target.cluster.clone();
            subscriptions.push(cx.subscribe(&manager, move |this, _, event: &ConnectionEvent, cx| {
                if matches!(event, ConnectionEvent::DiscoveryChanged(id) | ConnectionEvent::StateChanged(id) if *id == cluster)
                {
                    this.sync_stores(cx);
                }
            }));
        }
        let mut filters = Filters {
            namespace: target.namespace.clone(),
            ..Filters::default()
        };
        // Opened for one kind (the explorer's GitRepositories row, `:gitrepo`).
        if let Some(kind) = FluxKind::from_plural(&target.gvr.group, &target.gvr.resource)
            && category.kinds().len() > 1
        {
            filters.kinds.insert(kind);
        }
        let focus = cx.focus_handle();
        subscriptions.push(cx.on_focus_in(&focus, window, |this: &mut Self, _, cx| {
            this.focused = true;
            this.publish_selection(cx);
        }));
        subscriptions.push(cx.on_focus_out(&focus, window, |this: &mut Self, _, _, _| {
            this.focused = false;
        }));
        let mut this = Self {
            cluster: target.cluster.clone(),
            category,
            stores: Vec::new(),
            all: Vec::new(),
            rows: Vec::new(),
            filters,
            sort: ("name".into(), true),
            selected: Vec::new(),
            anchor: None,
            filter_input,
            focus,
            focused: false,
            scroll: UniformListScrollHandle::new(),
            parsed: state::ParsedObjects::default(),
            _store_observers: Vec::new(),
            _subscriptions: subscriptions,
        };
        this.sync_stores(cx);
        this
    }

    /// Watches every served kind of the category in all namespaces (the namespace filter is
    /// applied here, so switching it doesn't start new watches).
    fn sync_stores(&mut self, cx: &mut Context<Self>) {
        let wanted: Vec<(FluxKind, Gvr)> = self
            .category
            .kinds()
            .into_iter()
            .filter_map(|kind| Some((kind, state::gvr(&self.cluster, kind, cx)?)))
            .collect();
        let current: Vec<(FluxKind, Gvr)> = self
            .stores
            .iter()
            .map(|(k, g, _)| (*k, g.clone()))
            .collect();
        if current == wanted {
            return;
        }
        self.stores = wanted
            .into_iter()
            .map(|(kind, gvr)| {
                let handle = ResourceStores::acquire(cx, state::all_key(&self.cluster, &gvr));
                (kind, gvr, handle)
            })
            .collect();
        self._store_observers = self
            .stores
            .iter()
            .map(|(_, _, store)| cx.observe(store.entity(), |this, _, cx| this.rebuild(cx)))
            .collect();
        self.rebuild(cx);
    }

    /// The rows of every object, after a store changed (only changed stores are re-parsed).
    fn rebuild(&mut self, cx: &mut Context<Self>) {
        let mut all = Vec::new();
        for (kind, _, store) in &self.stores {
            all.extend(
                self.parsed
                    .of(*kind, store.entity(), cx)
                    .iter()
                    .cloned()
                    .map(Row::new),
            );
        }
        self.all = all;
        self.refresh(cx);
    }

    /// The shown rows after a filter or the sort changed (no parsing).
    fn refresh(&mut self, cx: &mut Context<Self>) {
        let mut rows: Vec<Row> = self
            .all
            .iter()
            .filter(|r| self.filters.matches(r))
            .cloned()
            .collect();
        rows::sort(&mut rows, &self.sort.0, self.sort.1);
        self.rows = rows;
        // Rows that went away leave the selection.
        let keys: BTreeSet<&str> = self.rows.iter().map(|r| r.key.as_ref()).collect();
        let before = self.selected.len();
        self.selected.retain(|k| keys.contains(k.as_ref()));
        if self.selected.len() != before || !self.selected.is_empty() {
            self.publish_selection(cx);
        }
        cx.notify();
    }

    /// `namespace/name` of the rows shown, in order.
    pub fn row_names(&self) -> Vec<String> {
        self.rows
            .iter()
            .map(|r| format!("{}/{}", r.namespace(), r.name()))
            .collect()
    }

    fn index_of(&self, key: &Arc<str>) -> Option<usize> {
        self.rows.iter().position(|r| &r.key == key)
    }

    fn primary_index(&self) -> Option<usize> {
        self.selected.first().and_then(|k| self.index_of(k))
    }

    fn row_ref(&self, row: &Row) -> Option<ResourceRef> {
        let gvr = self
            .stores
            .iter()
            .find(|(k, _, _)| *k == row.kind())
            .map(|(_, g, _)| g.clone())?;
        Some(ResourceRef::object(
            self.cluster.clone(),
            gvr,
            Some(row.namespace().to_string()),
            row.name().to_string(),
        ))
    }

    /// Selects one row (plain click, keys).
    fn select(&mut self, index: Option<usize>, cx: &mut Context<Self>) {
        self.selected = index
            .and_then(|i| self.rows.get(i))
            .map(|r| vec![r.key.clone()])
            .unwrap_or_default();
        self.anchor = self.selected.first().cloned();
        if let Some(index) = index {
            self.scroll.scroll_to_item(index, ScrollStrategy::Nearest);
        }
        self.publish_selection(cx);
        cx.notify();
    }

    /// ⌘/ctrl-click toggles a row, shift-click selects the range from the anchor.
    fn click(&mut self, index: usize, event: &ClickEvent, cx: &mut Context<Self>) {
        let Some(key) = self.rows.get(index).map(|r| r.key.clone()) else {
            return;
        };
        let modifiers = event.modifiers();
        if modifiers.shift
            && let Some(anchor) = self.anchor.clone().and_then(|a| self.index_of(&a))
        {
            let (from, to) = (anchor.min(index), anchor.max(index));
            let mut keys: Vec<Arc<str>> =
                self.rows[from..=to].iter().map(|r| r.key.clone()).collect();
            // The clicked row is the primary one.
            keys.retain(|k| *k != key);
            keys.insert(0, key);
            self.selected = keys;
        } else if modifiers.secondary() {
            if let Some(at) = self.selected.iter().position(|k| *k == key) {
                self.selected.remove(at);
            } else {
                self.selected.insert(0, key.clone());
            }
            self.anchor = Some(key);
        } else {
            self.select(Some(index), cx);
            return;
        }
        self.publish_selection(cx);
        cx.notify();
    }

    /// The details dock and the actions follow the selection, while the table has focus.
    fn publish_selection(&mut self, cx: &mut Context<Self>) {
        if !self.focused {
            return;
        }
        let caps = state::cluster_caps(&self.cluster, cx);
        let items: Vec<Selected> = self
            .selected
            .iter()
            .filter_map(|key| {
                let row = self.rows.iter().find(|r| &r.key == key)?;
                let target = self.row_ref(row)?;
                let store = self
                    .stores
                    .iter()
                    .find(|(k, _, _)| *k == row.kind())
                    .map(|(_, _, s)| s.entity().clone());
                Some(Selected {
                    target,
                    kind: row.kind().kind().to_string(),
                    object: Some(row.object.raw.clone()),
                    store,
                })
            })
            .collect();
        // Also when nothing is selected (any more, e.g. filtered out): the dock and the keys
        // must not act on a row the list no longer shows.
        ResourceSelection::set(cx, ResourceSelection { items, caps });
    }

    /// Whether the table has focus now (keys reach it only then).
    fn note_focus(&mut self, window: &Window, cx: &App) {
        self.focused = self.focus.contains_focused(window, cx);
    }

    fn move_selection(&mut self, movement: Move, cx: &mut Context<Self>) {
        let next = nav::step(self.primary_index(), self.rows.len(), movement);
        self.select(next, cx);
    }

    fn open_selected(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Some(target) = self
            .primary_index()
            .and_then(|i| self.rows.get(i))
            .and_then(|r| self.row_ref(r))
        else {
            return;
        };
        actions::open_object(target, window, cx);
    }

    fn set_sort(&mut self, column: SharedString, cx: &mut Context<Self>) {
        if self.sort.0 == column {
            self.sort.1 = !self.sort.1;
        } else {
            self.sort = (column, true);
        }
        self.refresh(cx);
    }

    fn toggle_state(&mut self, state: State, cx: &mut Context<Self>) {
        if !self.filters.states.remove(&state) {
            self.filters.states.insert(state);
        }
        self.refresh(cx);
    }

    /// Selects rows by state (tests, "select all failing").
    pub fn set_states(&mut self, states: impl IntoIterator<Item = State>, cx: &mut Context<Self>) {
        self.filters.states = states.into_iter().collect();
        self.refresh(cx);
    }

    /// The key-hint bar: writing actions only where they apply (none on read-only clusters).
    pub fn hints(&self, cx: &App) -> Vec<(SharedString, SharedString)> {
        let caps = state::cluster_caps(&self.cluster, cx);
        let row = self.primary_index().and_then(|i| self.rows.get(i));
        let primary = row.and_then(|r| self.row_ref(r));
        let mut hints: Vec<(SharedString, SharedString)> = vec![("enter".into(), "Open".into())];
        hints.extend(super::hints(
            LIST_CONTEXT,
            primary.as_ref(),
            row.map(|r| &r.object),
            &caps,
            cx,
        ));
        hints.push(("/".into(), "Filter".into()));
        hints
    }

    /// Selects the row of `namespace/name` (tests, links).
    pub fn select_name(&mut self, key: &str, cx: &mut Context<Self>) {
        let index = self
            .rows
            .iter()
            .position(|r| format!("{}/{}", r.namespace(), r.name()) == key);
        self.select(index, cx);
    }

    fn columns(&self) -> Vec<ColumnDef> {
        let flex = |weight: f32, min: f32| ColumnWidth::Flex { weight, min };
        let mut columns = vec![ColumnDef::new("name", "Name", flex(1.0, 120.0))];
        if self.category.kinds().len() > 1 {
            columns.push(ColumnDef::new("kind", "Kind", ColumnWidth::Fixed(118.0)));
        }
        columns.extend([
            ColumnDef::new("namespace", "Namespace", ColumnWidth::Fixed(96.0)),
            ColumnDef::new("state", "State", flex(1.6, 170.0)),
            ColumnDef::new("source", "Source", flex(1.0, 110.0)),
            ColumnDef::new("revision", "Revision", ColumnWidth::Fixed(150.0)),
            ColumnDef::new("interval", "Interval", ColumnWidth::Fixed(62.0)),
            ColumnDef::new("last", "Last", ColumnWidth::Fixed(54.0)),
            ColumnDef::new("age", "Age", ColumnWidth::Fixed(48.0)),
        ]);
        columns
    }

    fn status_label(&self, cx: &App) -> Option<String> {
        if self.stores.is_empty() {
            return Some(format!(
                "This cluster doesn't serve Flux {} (anymore).",
                self.category.label()
            ));
        }
        let statuses: Vec<StoreStatus> = self
            .stores
            .iter()
            .map(|(_, _, s)| s.read(cx).status().clone())
            .collect();
        if statuses
            .iter()
            .any(|s| matches!(s, StoreStatus::Waiting | StoreStatus::Loading))
        {
            return Some(format!("Loading {}…", self.category.label()));
        }
        if statuses.iter().all(|s| matches!(s, StoreStatus::Forbidden)) {
            return Some(format!(
                "You may not list Flux {} in all namespaces.",
                self.category.label()
            ));
        }
        statuses.iter().find_map(|s| match s {
            StoreStatus::Error(err) => Some(format!("Watch failed: {err}")),
            _ => None,
        })
    }

    // ----- Rendering -----

    fn render_toolbar(
        &mut self,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> impl IntoElement + use<> {
        let colors = cx.colors().clone();
        let total = self.all.len();
        let shown = self.rows.len();
        let failing = self.all.iter().filter(|r| r.state.is_problem()).count();
        let suspended = self
            .all
            .iter()
            .filter(|r| r.state == State::Suspended)
            .count();
        let count = if shown == total {
            total.to_string()
        } else {
            format!("{shown} of {total}")
        };
        let crumb = h_flex()
            .flex_none()
            .gap(u(6.0))
            .text_color(colors.text_dim)
            .child(Icon::new(widgets::category_icon(self.category)).color(colors.accent))
            .child(
                div()
                    .text_color(colors.text)
                    .font_weight(FontWeight::MEDIUM)
                    .child(self.category.label()),
            )
            .child("·")
            .child(count)
            .when(failing > 0, |this| {
                this.child(
                    div()
                        .text_color(colors.red)
                        .child(format!("· {failing} failing")),
                )
            })
            .when(suspended > 0, |this| {
                this.child(
                    div()
                        .text_color(colors.purple)
                        .child(format!("· {suspended} suspended")),
                )
            });

        let weak = cx.entity().downgrade();
        let namespaces = rows::namespaces(&self.all);
        let current_ns = self.filters.namespace.clone();
        let ns_label = match &current_ns {
            Some(ns) => format!("Namespace: {ns}"),
            None => "All namespaces".to_string(),
        };
        let namespace_menu = MenuButton::new("flux-namespace")
            .ghost()
            .compact()
            .child(
                h_flex()
                    .gap(u(4.0))
                    .text_size(u(12.0))
                    .text_color(if current_ns.is_some() {
                        colors.chip_selected_text
                    } else {
                        colors.text_muted
                    })
                    .child(ns_label)
                    .child(Icon::new(IconName::ChevronDown).size(11.0)),
            )
            .dropdown_menu({
                let weak = weak.clone();
                move |menu, _, _| {
                    let mut menu = menu.max_h(gpui::px(420.0)).scrollable(true);
                    let all = weak.clone();
                    menu = menu.item(
                        PopupMenuItem::new("All namespaces")
                            .checked(current_ns.is_none())
                            .on_click(move |_, _, cx| {
                                all.update(cx, |this, cx| {
                                    this.filters.namespace = None;
                                    this.refresh(cx);
                                })
                                .ok();
                            }),
                    );
                    menu = menu.separator();
                    for ns in &namespaces {
                        let weak = weak.clone();
                        let value = ns.clone();
                        menu = menu.item(
                            PopupMenuItem::new(ns.clone())
                                .checked(current_ns.as_deref() == Some(ns.as_str()))
                                .on_click(move |_, _, cx| {
                                    let value = value.clone();
                                    weak.update(cx, |this, cx| {
                                        this.filters.namespace = Some(value);
                                        this.refresh(cx);
                                    })
                                    .ok();
                                }),
                        );
                    }
                    menu
                }
            });
        let kinds: Vec<FluxKind> = self.stores.iter().map(|(k, _, _)| *k).collect();
        let kind_menu = (kinds.len() > 1).then(|| {
            let current = self.filters.kinds.clone();
            let label = match current.iter().next() {
                Some(kind) if current.len() == 1 => format!("Kind: {}", kind.kind()),
                _ => "All kinds".to_string(),
            };
            MenuButton::new("flux-kind")
                .ghost()
                .compact()
                .child(
                    h_flex()
                        .gap(u(4.0))
                        .text_size(u(12.0))
                        .text_color(if current.is_empty() {
                            colors.text_muted
                        } else {
                            colors.chip_selected_text
                        })
                        .child(label)
                        .child(Icon::new(IconName::ChevronDown).size(11.0)),
                )
                .dropdown_menu({
                    let weak = weak.clone();
                    move |menu, _, _| {
                        let all = weak.clone();
                        let mut menu = menu.item(
                            PopupMenuItem::new("All kinds")
                                .checked(current.is_empty())
                                .on_click(move |_, _, cx| {
                                    all.update(cx, |this, cx| {
                                        this.filters.kinds.clear();
                                        this.refresh(cx);
                                    })
                                    .ok();
                                }),
                        );
                        menu = menu.separator();
                        for kind in &kinds {
                            let weak = weak.clone();
                            let kind = *kind;
                            menu = menu.item(
                                PopupMenuItem::new(kind.label())
                                    .checked(current.contains(&kind))
                                    .on_click(move |_, _, cx| {
                                        weak.update(cx, |this, cx| {
                                            this.filters.kinds = [kind].into();
                                            this.refresh(cx);
                                        })
                                        .ok();
                                    }),
                            );
                        }
                        menu
                    }
                })
        });

        let filter_focused = self
            .filter_input
            .read(cx)
            .focus_handle(cx)
            .is_focused(window);
        let filter = div()
            .flex_none()
            .w(u(170.0))
            .h(u(sizes::CONTROL))
            .px(u(8.0))
            .flex()
            .items_center()
            .gap(u(7.0))
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
                    .child(Input::new(&self.filter_input).appearance(false)),
            );
        let live = self
            .stores
            .first()
            .map(|(_, _, s)| s.read(cx).status().clone());
        let (live_label, live_color) = match live {
            Some(StoreStatus::Ready) => ("live", colors.green),
            Some(StoreStatus::Paused) => ("paused", colors.yellow),
            Some(StoreStatus::Error(_)) => ("retrying", colors.red),
            _ => ("…", colors.text_dim),
        };
        h_flex()
            .flex_none()
            .h(u(sizes::TOOLBAR))
            .px(u(12.0))
            .gap(u(8.0))
            .border_b_1()
            .border_color(colors.border_variant)
            .child(crumb)
            .child(div().flex_1())
            .children(kind_menu)
            .child(namespace_menu)
            .child(filter)
            .child(Chip::new(live_label).dot(live_color).text_color(live_color))
    }

    fn render_chips(&self, cx: &mut Context<Self>) -> impl IntoElement + use<> {
        let colors = cx.colors().clone();
        let scope: Vec<&Row> = self
            .all
            .iter()
            .filter(|r| self.filters.matches_scope(r))
            .collect();
        let counts = rows::state_counts(scope);
        let mut row = h_flex()
            .flex_none()
            .h(u(36.0))
            .px(u(12.0))
            .gap(u(6.0))
            .border_b_1()
            .border_color(colors.border_variant)
            .overflow_hidden();
        for (state, count) in counts {
            let selected = self.filters.states.contains(&state);
            if count == 0 && !selected && state != State::Ready {
                continue;
            }
            row = row.child(
                h_flex()
                    .id(SharedString::from(format!("flux-state-{}", state.label())))
                    .gap(u(4.0))
                    .cursor_pointer()
                    .on_click(cx.listener(move |this, _, _, cx| this.toggle_state(state, cx)))
                    .child(
                        Chip::new(state.label())
                            .dot(widgets::state_color(state, &colors))
                            .selected(selected),
                    )
                    .child(widgets::count(count, &colors)),
            );
        }
        let filtered = !self.filters.states.is_empty();
        let selected = self.selected.len();
        row.child(div().flex_1())
            .when(selected > 1, |this| {
                this.child(
                    div()
                        .text_size(u(12.0))
                        .text_color(colors.text_muted)
                        .child(format!("{selected} selected")),
                )
            })
            .when(filtered, |this| {
                this.child(
                    div()
                        .id("flux-clear-states")
                        .text_size(u(12.0))
                        .text_color(colors.accent)
                        .cursor_pointer()
                        .on_click(cx.listener(|this, _, _, cx| {
                            this.filters.states.clear();
                            this.refresh(cx);
                        }))
                        .child("Clear"),
                )
            })
    }

    fn render_header(&self, cx: &mut Context<Self>) -> impl IntoElement + use<> {
        let colors = cx.colors().clone();
        let sort = self.sort.clone();
        h_flex()
            .flex_none()
            .h(u(sizes::TABLE_HEADER))
            .px(u(12.0))
            .bg(colors.subheader_background)
            .border_b_1()
            .border_color(colors.border_variant)
            .text_size(u(11.5))
            .text_color(colors.text_dim)
            .whitespace_nowrap()
            .overflow_hidden()
            .children(self.columns().into_iter().map(|def| {
                let id = def.id.clone();
                let sorted = (sort.0 == def.id).then_some(sort.1);
                widgets::column_cell(&def).child(
                    h_flex()
                        .id(SharedString::from(format!("flux-sort-{id}")))
                        .gap(u(3.0))
                        .cursor_pointer()
                        .when(sorted.is_some(), |this| this.text_color(colors.text_muted))
                        .child(def.title.to_uppercase())
                        .when_some(sorted, |this, ascending| {
                            this.child(
                                Icon::new(if ascending {
                                    IconName::ChevronDown
                                } else {
                                    IconName::ArrowUp
                                })
                                .size(10.0)
                                .color(colors.text_dim),
                            )
                        })
                        .on_click(cx.listener(move |this, _, _, cx| this.set_sort(id.clone(), cx))),
                )
            }))
    }

    fn render_rows(
        &mut self,
        range: Range<usize>,
        cx: &mut Context<Self>,
    ) -> Vec<gpui::AnyElement> {
        let colors = cx.colors().clone();
        let columns = self.columns();
        range
            .filter_map(|index| {
                let row = self.rows.get(index)?.clone();
                let selected = self.selected.contains(&row.key);
                let element = widgets::row(("flux-row", index), selected, ROW_HEIGHT, &colors)
                    .children(columns.iter().map(|def| {
                        widgets::column_cell(def).child(cell(&row, def.id.as_ref(), &colors))
                    }))
                    .on_click(cx.listener(move |this, event: &ClickEvent, window, cx| {
                        this.focus.focus(window, cx);
                        // Focus listeners only run on the next draw (and not while the window
                        // is inactive): a click on a row is focus enough.
                        this.focused = true;
                        this.click(index, event, cx);
                        if event.click_count() == 2 {
                            this.open_selected(window, cx);
                        }
                    }));
                Some(element.into_any_element())
            })
            .collect()
    }
}

/// One cell of a row.
fn cell(row: &Row, column: &str, colors: &Colors) -> gpui::AnyElement {
    let object = &row.object;
    match column {
        "name" => h_flex()
            .min_w_0()
            .gap(u(5.0))
            .font_family(fonts::MONO)
            .text_size(u(12.0))
            .child(div().truncate().child(row.name().to_string()))
            .when(object.suspended, |this| {
                this.child(Icon::new(IconName::Pause).size(11.0).color(colors.purple))
            })
            .into_any_element(),
        "kind" => div()
            .truncate()
            .text_color(colors.text_muted)
            .child(row.kind().kind())
            .into_any_element(),
        "namespace" => div()
            .truncate()
            .text_color(colors.text_muted)
            .child(row.namespace().to_string())
            .into_any_element(),
        "state" => h_flex()
            .min_w_0()
            .gap(u(8.0))
            .child(
                div()
                    .flex_none()
                    .child(widgets::state_pill(row.state, colors)),
            )
            .child(
                div()
                    .min_w_0()
                    .truncate()
                    .text_size(u(12.0))
                    .text_color(if row.state.is_problem() {
                        colors.text_muted
                    } else {
                        colors.text_dim
                    })
                    .child(row.message.clone()),
            )
            .into_any_element(),
        "source" => widgets::mono(row.source.clone()),
        "revision" => widgets::mono(row.revision.clone()),
        "interval" => div()
            .font_family(fonts::MONO)
            .text_size(u(11.5))
            .text_color(colors.text_muted)
            .child(object.interval.clone().unwrap_or_default())
            .into_any_element(),
        "last" => div()
            .font_family(fonts::MONO)
            .text_size(u(11.5))
            .text_color(colors.text_muted)
            .child(widgets::age(object.last_reconcile().as_deref()))
            .into_any_element(),
        "age" => div()
            .font_family(fonts::MONO)
            .text_size(u(11.5))
            .text_color(colors.text_muted)
            .child(widgets::age(object.created.as_deref()))
            .into_any_element(),
        _ => div().into_any_element(),
    }
}

impl Focusable for ListView {
    fn focus_handle(&self, _: &App) -> FocusHandle {
        self.focus.clone()
    }
}

impl TabView for ListView {
    fn tab_title(&self, _: &App) -> SharedString {
        match self.filters.kinds.iter().next() {
            Some(kind) if self.filters.kinds.len() == 1 => kind.label().into(),
            _ => self.category.label().into(),
        }
    }

    fn tab_icon(&self, _: &App) -> Option<SharedString> {
        Some(widgets::category_icon(self.category).path())
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
        // A list of one kind (GitRepositories) comes back as that kind.
        let mut target = super::category_ref(&self.cluster, self.filters.namespace.clone());
        if self.filters.kinds.len() == 1
            && let Some(kind) = self.filters.kinds.iter().next()
            && let Some((_, gvr, _)) = self.stores.iter().find(|(k, _, _)| k == kind)
        {
            target.gvr = gvr.clone();
        }
        Some(ViewRequest::for_resource(
            ViewKind::Custom(self.category.view_id().into()),
            target,
        ))
    }
}

impl Render for ListView {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let colors = cx.colors().clone();
        let toolbar = self.render_toolbar(window, cx);
        let chips = self.render_chips(cx);
        let header = self.render_header(cx);
        let body = if self.rows.is_empty() {
            let message = self.status_label(cx).unwrap_or_else(|| {
                if self.all.is_empty() {
                    format!("No {}.", self.category.label())
                } else {
                    format!("No {} match the filters.", self.category.label())
                }
            });
            widgets::empty(message, &colors)
        } else {
            uniform_list(
                "flux-rows",
                self.rows.len(),
                cx.processor(|this, range: Range<usize>, _, cx| this.render_rows(range, cx)),
            )
            .flex_1()
            .track_scroll(&self.scroll)
            .into_any_element()
        };
        let hints = self.hints(cx);
        v_flex()
            .key_context(LIST_CONTEXT)
            .size_full()
            .bg(colors.background)
            .text_color(colors.text)
            .on_action(cx.listener(|this, _: &super::FocusFilter, window, cx| {
                let focus = this.filter_input.read(cx).focus_handle(cx);
                focus.focus(window, cx);
            }))
            .child(toolbar)
            .child(chips)
            .child(
                v_flex()
                    .id("flux-table")
                    .key_context(TABLE)
                    .track_focus(&self.focus)
                    .flex_1()
                    .min_h_0()
                    .on_action(cx.listener(|this, _: &nav::SelectNext, window, cx| {
                        this.note_focus(window, cx);
                        this.move_selection(Move::Next, cx)
                    }))
                    .on_action(cx.listener(|this, _: &nav::SelectPrevious, window, cx| {
                        this.note_focus(window, cx);
                        this.move_selection(Move::Previous, cx)
                    }))
                    .on_action(cx.listener(|this, _: &nav::SelectFirst, window, cx| {
                        this.note_focus(window, cx);
                        this.move_selection(Move::First, cx)
                    }))
                    .on_action(cx.listener(|this, _: &nav::SelectLast, window, cx| {
                        this.note_focus(window, cx);
                        this.move_selection(Move::Last, cx)
                    }))
                    .on_action(cx.listener(|this, _: &nav::SelectPageDown, window, cx| {
                        this.note_focus(window, cx);
                        this.move_selection(Move::PageDown, cx)
                    }))
                    .on_action(cx.listener(|this, _: &nav::SelectPageUp, window, cx| {
                        this.note_focus(window, cx);
                        this.move_selection(Move::PageUp, cx)
                    }))
                    .on_action(cx.listener(|this, _: &nav::Confirm, window, cx| {
                        this.open_selected(window, cx)
                    }))
                    .child(header)
                    .child(body),
            )
            .child(KeyHints::new(hints))
            .font_family(fonts::UI)
    }
}
