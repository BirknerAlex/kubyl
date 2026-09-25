//! The Applications view (board 12): every Application of a cluster with sync and health, the
//! running operation, auto-sync, source, revision, destination (a link when it's one of your
//! contexts) and the last sync; filters by sync, health, project and destination; the details
//! dock follows the selection.

use std::ops::Range;
use std::sync::Arc;

use gpui::{
    App, AppContext as _, Context, Entity, FocusHandle, Focusable, FontWeight, IntoElement, Render,
    ScrollStrategy, SharedString, Subscription, UniformListScrollHandle, Window, div, prelude::*,
    uniform_list,
};
use gpui_component::button::{Button as MenuButton, ButtonVariants as _};
use gpui_component::input::{Input, InputEvent, InputState};
use gpui_component::menu::{DropdownMenu as _, PopupMenuItem};
use kubyl_core::actions::OpenView;
use kubyl_core::{
    ActionRegistry, ActiveContext, ClusterId, ColumnDef, ColumnWidth, Gvr, ResourceRef, TabView,
    ViewKind, ViewRequest,
};
use kubyl_kube::{ConnectionEvent, ConnectionManager};
use kubyl_resources::{ResourceSelection, ResourceStores, Selected, StoreHandle, StoreStatus};
use kubyl_ui::{
    ActiveColors, Chip, Colors, Icon, IconButton, IconName, KeyHints, fonts, h_flex, sizes, u,
    v_flex,
};

use crate::actions::{self, APPS_CONTEXT};
use crate::apps::{self, AppRow, Filters};
use crate::model::{Health, SyncStatus};
use crate::nav::{self, Move, TABLE};
use crate::state::{self, ArgoCd};
use crate::widgets::{self, Sort};

/// `ViewKind::Custom` of the Applications view.
pub const VIEW_KIND: &str = "argocd_apps";
const ROW_HEIGHT: f32 = 32.0;

/// Opens the Applications view of `cluster` (in `namespace`, else all namespaces).
pub fn open(cluster: &ClusterId, namespace: Option<String>, window: &mut Window, cx: &mut App) {
    let gvr = state::resource(cluster, "applications", cx)
        .map(|(gvr, _)| gvr)
        .unwrap_or_else(|| Gvr::new(crate::model::GROUP, "v1alpha1", "applications"));
    window.dispatch_action(
        Box::new(OpenView(ViewRequest::for_resource(
            ViewKind::Custom(VIEW_KIND.into()),
            ResourceRef::list(cluster.clone(), gvr, namespace),
        ))),
        cx,
    );
}

fn columns() -> Vec<ColumnDef> {
    let flex = |weight: f32, min: f32| ColumnWidth::Flex { weight, min };
    // Fits next to the details dock at 1440 px.
    vec![
        ColumnDef::new("name", "Name", flex(1.3, 120.0)),
        ColumnDef::new("project", "Project", ColumnWidth::Fixed(84.0)),
        ColumnDef::new("sync", "Sync", ColumnWidth::Fixed(110.0)),
        ColumnDef::new("health", "Health", ColumnWidth::Fixed(102.0)),
        ColumnDef::new("auto", "Auto", ColumnWidth::Fixed(42.0)),
        ColumnDef::new("source", "Source @ Target", flex(1.5, 120.0)),
        ColumnDef::new("revision", "Revision", ColumnWidth::Fixed(68.0)),
        ColumnDef::new("destination", "Destination", flex(1.0, 100.0)),
        ColumnDef::new("last", "Last", ColumnWidth::Fixed(58.0)),
    ]
}

pub struct AppsView {
    cluster: ClusterId,
    /// `None`: all namespaces.
    namespace: Option<String>,
    gvr: Option<Gvr>,
    stores: Vec<StoreHandle>,
    all: Vec<AppRow>,
    rows: Vec<AppRow>,
    filters: Filters,
    sort: Sort,
    selected: Option<Arc<str>>,
    filter_input: Entity<InputState>,
    focus: FocusHandle,
    scroll: UniformListScrollHandle,
    _store_observers: Vec<Subscription>,
    _subscriptions: Vec<Subscription>,
}

impl AppsView {
    pub fn new(target: ResourceRef, window: &mut Window, cx: &mut Context<Self>) -> Self {
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
        if let Some(argo) = ArgoCd::try_global(cx) {
            subscriptions.push(cx.observe(&argo, |this, _, cx| {
                // Detection finished: the fallback namespaces may be known now.
                this.sync_stores(cx);
                cx.notify();
            }));
        }
        let mut this = Self {
            cluster: target.cluster.clone(),
            namespace: target.namespace.clone(),
            gvr: None,
            stores: Vec::new(),
            all: Vec::new(),
            rows: Vec::new(),
            filters: Filters::default(),
            sort: Sort {
                column: "name".into(),
                ascending: true,
            },
            selected: None,
            filter_input,
            focus: cx.focus_handle(),
            scroll: UniformListScrollHandle::new(),
            _store_observers: Vec::new(),
            _subscriptions: subscriptions,
        };
        this.sync_stores(cx);
        this
    }

    /// Watches the Applications: all namespaces, or (when that's forbidden) the namespaces
    /// Argo CD manages.
    fn sync_stores(&mut self, cx: &mut Context<Self>) {
        let gvr = state::resource(&self.cluster, "applications", cx).map(|(gvr, _)| gvr);
        let Some(gvr) = gvr else {
            if self.gvr.take().is_some() || !self.stores.is_empty() {
                self.stores.clear();
                self._store_observers.clear();
                self.refresh(cx);
            }
            return;
        };
        let mut keys = vec![match &self.namespace {
            Some(ns) => apps::namespace_key(&self.cluster, &gvr, ns),
            None => apps::all_key(&self.cluster, &gvr),
        }];
        let forbidden = self
            .stores
            .first()
            .is_some_and(|s| matches!(s.read(cx).status(), StoreStatus::Forbidden));
        if forbidden && self.namespace.is_none() {
            let installs = ArgoCd::try_global(cx)
                .map(|a| a.read(cx).installs(&self.cluster))
                .unwrap_or_default();
            for install in installs.iter() {
                for ns in std::iter::once(&install.namespace).chain(install.app_namespaces.iter()) {
                    if !ns.contains('*') && !ns.contains('?') {
                        keys.push(apps::namespace_key(&self.cluster, &gvr, ns));
                    }
                }
            }
        }
        let current: Vec<_> = self
            .stores
            .iter()
            .map(|s| s.read(cx).key().clone())
            .collect();
        if self.gvr.as_ref() == Some(&gvr) && current == keys {
            return;
        }
        self.gvr = Some(gvr);
        self.stores = keys
            .into_iter()
            .map(|key| ResourceStores::acquire(cx, key))
            .collect();
        self._store_observers = self
            .stores
            .iter()
            .map(|store| {
                cx.observe(store.entity(), |this, store, cx| {
                    let forbidden = matches!(store.read(cx).status(), StoreStatus::Forbidden);
                    if forbidden && this.stores.len() == 1 {
                        this.sync_stores(cx);
                    }
                    this.refresh(cx);
                })
            })
            .collect();
        self.refresh(cx);
    }

    fn refresh(&mut self, cx: &mut Context<Self>) {
        let known = apps::known_clusters(cx);
        let mut seen = std::collections::HashSet::new();
        let mut all = Vec::new();
        for store in &self.stores {
            for object in store.read(cx).objects().values() {
                if let Some(row) = AppRow::new(object.clone(), &self.cluster, &known)
                    && seen.insert(row.key.clone())
                {
                    all.push(row);
                }
            }
        }
        self.all = all;
        let mut rows: Vec<AppRow> = self
            .all
            .iter()
            .filter(|r| self.filters.matches(r))
            .cloned()
            .collect();
        apps::sort_rows(&mut rows, &self.sort.column, self.sort.ascending);
        self.rows = rows;
        cx.notify();
    }

    fn selected_index(&self) -> Option<usize> {
        let key = self.selected.as_ref()?;
        self.rows.iter().position(|r| &r.key == key)
    }

    fn row_ref(&self, row: &AppRow) -> Option<ResourceRef> {
        Some(ResourceRef::object(
            self.cluster.clone(),
            self.gvr.clone()?,
            Some(row.namespace().to_string()),
            row.name().to_string(),
        ))
    }

    fn select(&mut self, index: Option<usize>, cx: &mut Context<Self>) {
        self.selected = index.and_then(|i| self.rows.get(i)).map(|r| r.key.clone());
        if let Some(index) = index {
            self.scroll.scroll_to_item(index, ScrollStrategy::Nearest);
        }
        self.publish_selection(cx);
        cx.notify();
    }

    /// The details dock and the actions follow the selection.
    fn publish_selection(&mut self, cx: &mut Context<Self>) {
        let Some(row) = self
            .selected_index()
            .and_then(|i| self.rows.get(i))
            .cloned()
        else {
            return;
        };
        let Some(target) = self.row_ref(&row) else {
            return;
        };
        let store = self
            .stores
            .iter()
            .find(|s| s.read(cx).get(&row.key).is_some())
            .map(|s| s.entity().clone());
        let caps = ConnectionManager::try_global(cx)
            .map(|m| m.read(cx).caps(&self.cluster))
            .unwrap_or_default();
        ResourceSelection::set(
            cx,
            ResourceSelection {
                items: vec![Selected {
                    target,
                    kind: "Application".into(),
                    object: Some(row.object.clone()),
                    store,
                }],
                caps,
            },
        );
    }

    fn move_selection(&mut self, movement: Move, cx: &mut Context<Self>) {
        let next = nav::step(self.selected_index(), self.rows.len(), movement);
        self.select(next, cx);
    }

    fn open_selected(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Some(target) = self
            .selected_index()
            .and_then(|i| self.rows.get(i))
            .and_then(|r| self.row_ref(r))
        else {
            return;
        };
        actions::open_app(target, None, window, cx);
    }

    fn set_sort(&mut self, column: SharedString, cx: &mut Context<Self>) {
        if self.sort.column == column {
            self.sort.ascending = !self.sort.ascending;
        } else {
            self.sort = Sort {
                column,
                ascending: true,
            };
        }
        self.refresh(cx);
    }

    fn toggle_sync(&mut self, status: SyncStatus, cx: &mut Context<Self>) {
        if !self.filters.sync.remove(&status) {
            self.filters.sync.insert(status);
        }
        self.refresh(cx);
    }

    fn toggle_health(&mut self, health: Health, cx: &mut Context<Self>) {
        if !self.filters.health.remove(&health) {
            self.filters.health.insert(health);
        }
        self.refresh(cx);
    }

    fn status_label(&self, cx: &App) -> Option<String> {
        let store = self.stores.first()?.read(cx);
        match store.status() {
            StoreStatus::Waiting | StoreStatus::Loading => Some("Loading applications…".into()),
            StoreStatus::Forbidden if self.stores.len() == 1 => {
                Some("You may not list Applications here.".into())
            }
            StoreStatus::Unsupported => {
                Some("This cluster doesn't serve Argo CD Applications (anymore).".into())
            }
            StoreStatus::Error(err) => Some(format!("Watch failed: {err}")),
            _ => None,
        }
    }

    // ----- Rendering -----

    fn render_toolbar(
        &mut self,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> impl IntoElement + use<> {
        let colors = cx.colors().clone();
        let (out_of_sync, degraded) = apps::problems(&self.all);
        let total = self.all.len();
        let shown = self.rows.len();
        let count = if shown == total {
            total.to_string()
        } else {
            format!("{shown} of {total}")
        };
        let crumb = h_flex()
            .flex_none()
            .gap(u(6.0))
            .text_color(colors.text_dim)
            .child(Icon::new(IconName::Layers).color(colors.accent))
            .child(
                div()
                    .text_color(colors.text)
                    .font_weight(FontWeight::MEDIUM)
                    .child("Applications"),
            )
            .child("·")
            .child(count)
            .when(out_of_sync > 0, |this| {
                this.child(
                    div()
                        .text_color(colors.yellow)
                        .child(format!("· {out_of_sync} out of sync")),
                )
            })
            .when(degraded > 0, |this| {
                this.child(
                    div()
                        .text_color(colors.red)
                        .child(format!("· {degraded} degraded")),
                )
            });

        let (projects, destinations) = apps::choices(&self.all);
        let weak = cx.entity().downgrade();
        let dropdown = |id: &'static str,
                        label: &'static str,
                        current: Option<String>,
                        choices: Vec<String>,
                        set: fn(&mut Filters, Option<String>)| {
            let weak = weak.clone();
            let title = match &current {
                Some(value) => format!("{label}: {value}"),
                None => label.to_string(),
            };
            MenuButton::new(id)
                .ghost()
                .compact()
                .child(
                    h_flex()
                        .gap(u(4.0))
                        .text_size(u(12.0))
                        .text_color(if current.is_some() {
                            colors.chip_selected_text
                        } else {
                            colors.text_muted
                        })
                        .child(widgets::middle_ellipsis(&title, 28))
                        .child(Icon::new(IconName::ChevronDown).size(11.0)),
                )
                .dropdown_menu(move |menu, _, _| {
                    let mut menu = menu.max_h(gpui::px(420.0)).scrollable(true);
                    let all = weak.clone();
                    menu = menu.item(
                        PopupMenuItem::new(format!("All {}", label.to_lowercase() + "s"))
                            .checked(current.is_none())
                            .on_click(move |_, _, cx| {
                                all.update(cx, |this, cx| {
                                    set(&mut this.filters, None);
                                    this.refresh(cx);
                                })
                                .ok();
                            }),
                    );
                    menu = menu.separator();
                    for choice in &choices {
                        let weak = weak.clone();
                        let value = choice.clone();
                        menu = menu.item(
                            PopupMenuItem::new(choice.clone())
                                .checked(current.as_deref() == Some(choice.as_str()))
                                .on_click(move |_, _, cx| {
                                    let value = value.clone();
                                    weak.update(cx, |this, cx| {
                                        set(&mut this.filters, Some(value));
                                        this.refresh(cx);
                                    })
                                    .ok();
                                }),
                        );
                    }
                    menu
                })
        };
        let project = dropdown(
            "argo-project",
            "Project",
            self.filters.project.clone(),
            projects,
            |f, v| f.project = v,
        );
        let destination = dropdown(
            "argo-destination",
            "Destination",
            self.filters.destination.clone(),
            destinations,
            |f, v| f.destination = v,
        );

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

        let api_state = ArgoCd::try_global(cx)
            .map(|a| a.read(cx).api_state(&self.cluster))
            .unwrap_or_default();
        let cluster = self.cluster.clone();
        let mode = super::mode_button("argo-apps-mode", &cluster, &api_state, cx);
        let ui_cluster = self.cluster.clone();
        let argo_ui = div()
            .id("argo-apps-ui")
            .tooltip(|window, cx| {
                gpui_component::tooltip::Tooltip::new("Open Argo CD UI").build(window, cx)
            })
            .child(
                IconButton::new("argo-apps-ui-button", IconName::Globe)
                    .icon_size(14.0)
                    .on_click(move |_, window, cx| actions::open_argo_ui(&ui_cluster, window, cx)),
            );
        let live = self.stores.first().map(|s| s.read(cx).status().clone());
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
            .child(project)
            .child(destination)
            .child(filter)
            .child(mode)
            .child(argo_ui)
            .child(Chip::new(live_label).dot(live_color).text_color(live_color))
    }

    fn render_chips(&self, cx: &mut Context<Self>) -> impl IntoElement + use<> {
        let colors = cx.colors().clone();
        let scope: Vec<&AppRow> = self
            .all
            .iter()
            .filter(|r| self.filters.matches_scope(r))
            .collect();
        let counts = apps::counts(scope);
        let chip = |id: SharedString, label: &'static str, count: usize, color, selected: bool| {
            Chip::new(label)
                .dot(color)
                .selected(selected)
                .into_any_element()
                .map_chip(id, count, &colors)
        };
        let mut row = h_flex()
            .flex_none()
            .h(u(36.0))
            .px(u(12.0))
            .gap(u(6.0))
            .border_b_1()
            .border_color(colors.border_variant)
            .overflow_hidden();
        for (status, count) in counts.sync {
            let selected = self.filters.sync.contains(&status);
            row = row.child(
                div()
                    .id(SharedString::from(format!("sync-chip-{}", status.label())))
                    .cursor_pointer()
                    .on_click(cx.listener(move |this, _, _, cx| this.toggle_sync(status, cx)))
                    .child(chip(
                        status.label().into(),
                        status.label(),
                        count,
                        widgets::sync_color(status, &colors),
                        selected,
                    )),
            );
        }
        row = row.child(div().w(u(1.0)).h(u(16.0)).mx(u(4.0)).bg(colors.border));
        for (health, count) in counts.health {
            let selected = self.filters.health.contains(&health);
            if count == 0 && !selected {
                continue;
            }
            row = row.child(
                div()
                    .id(SharedString::from(format!(
                        "health-chip-{}",
                        health.label()
                    )))
                    .cursor_pointer()
                    .on_click(cx.listener(move |this, _, _, cx| this.toggle_health(health, cx)))
                    .child(chip(
                        health.label().into(),
                        health.label(),
                        count,
                        widgets::health_color(health, &colors),
                        selected,
                    )),
            );
        }
        let filtered = !self.filters.sync.is_empty() || !self.filters.health.is_empty();
        row.child(div().flex_1()).when(filtered, |this| {
            this.child(
                div()
                    .id("clear-status-filters")
                    .text_size(u(12.0))
                    .text_color(colors.accent)
                    .cursor_pointer()
                    .on_click(cx.listener(|this, _, _, cx| {
                        this.filters.sync.clear();
                        this.filters.health.clear();
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
            .children(columns().into_iter().map(|def| {
                let id = def.id.clone();
                let sorted = (sort.column == def.id).then_some(sort.ascending);
                widgets::column_cell(&def).child(
                    h_flex()
                        .id(SharedString::from(format!("argo-sort-{id}")))
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
        let columns = columns();
        let selected = self.selected_index();
        let own_namespaces: Vec<String> = ArgoCd::try_global(cx)
            .map(|a| {
                a.read(cx)
                    .installs(&self.cluster)
                    .iter()
                    .map(|i| i.namespace.clone())
                    .collect()
            })
            .unwrap_or_default();
        range
            .filter_map(|index| {
                let row = self.rows.get(index)?.clone();
                let element = widgets::row(
                    ("argo-app", index),
                    selected == Some(index),
                    ROW_HEIGHT,
                    &colors,
                )
                .children(columns.iter().map(|def| {
                    widgets::column_cell(def).child(self.cell(
                        &row,
                        def.id.as_ref(),
                        &own_namespaces,
                        &colors,
                        cx,
                    ))
                }))
                .on_click(cx.listener(
                    move |this, event: &gpui::ClickEvent, window, cx| {
                        this.focus.focus(window, cx);
                        this.select(Some(index), cx);
                        if event.click_count() == 2 {
                            this.open_selected(window, cx);
                        }
                    },
                ));
                Some(element.into_any_element())
            })
            .collect()
    }

    fn cell(
        &self,
        row: &AppRow,
        column: &str,
        own_namespaces: &[String],
        colors: &Colors,
        cx: &mut Context<Self>,
    ) -> gpui::AnyElement {
        let app = &row.app;
        match column {
            "name" => {
                let foreign = !own_namespaces.is_empty()
                    && !own_namespaces.iter().any(|n| n == row.namespace());
                h_flex()
                    .min_w_0()
                    .gap(u(4.0))
                    .font_family(fonts::MONO)
                    .text_size(u(12.0))
                    .child(div().truncate().child(row.name().to_string()))
                    .when(foreign, |this| {
                        this.child(
                            div()
                                .flex_none()
                                .text_color(colors.text_dim)
                                .child(format!("· {}", row.namespace())),
                        )
                    })
                    .into_any_element()
            }
            "project" => div()
                .truncate()
                .text_color(colors.text_muted)
                .child(row.project().to_string())
                .into_any_element(),
            "sync" => match app.activity() {
                Some(activity) => widgets::activity_chip(&activity, colors).into_any_element(),
                None => widgets::sync_pill(app.sync(), colors).into_any_element(),
            },
            "health" => widgets::health_pill(app.health(), colors).into_any_element(),
            "auto" => {
                let policy = app.policy();
                if policy.auto_sync() {
                    let letters = format!(
                        "{}{}",
                        if policy.prune() { "P" } else { "" },
                        if policy.self_heal() { "H" } else { "" }
                    );
                    h_flex()
                        .gap(u(3.0))
                        .child(
                            Icon::new(IconName::RefreshCw)
                                .size(12.0)
                                .color(colors.green),
                        )
                        .child(
                            div()
                                .font_family(fonts::MONO)
                                .text_size(u(10.5))
                                .text_color(colors.text_dim)
                                .child(letters),
                        )
                        .into_any_element()
                } else {
                    div()
                        .text_size(u(12.0))
                        .text_color(colors.text_faint)
                        .child("manual")
                        .into_any_element()
                }
            }
            "source" => {
                let (what, target) = row
                    .source
                    .split_once(" @ ")
                    .map(|(a, b)| (a.to_string(), Some(b.to_string())))
                    .unwrap_or((row.source.clone(), None));
                h_flex()
                    .min_w_0()
                    .gap(u(4.0))
                    .child(
                        div()
                            .truncate()
                            .font_family(fonts::MONO)
                            .text_size(u(11.5))
                            .child(what),
                    )
                    .when_some(target, |this, target| {
                        this.child(
                            div()
                                .flex_none()
                                .text_size(u(12.0))
                                .text_color(colors.text_dim)
                                .child(format!("@ {target}")),
                        )
                    })
                    .into_any_element()
            }
            "revision" => widgets::mono(row.revision.clone()),
            "destination" => match row.destination_cluster.clone() {
                Some(cluster) => {
                    let namespace = app.spec.destination.namespace.clone();
                    widgets::link(
                        SharedString::from(format!("argo-dest-{}", row.key)),
                        row.destination.clone(),
                        colors,
                        move |_, window, cx| {
                            super::open_destination(&cluster, namespace.clone(), window, cx)
                        },
                    )
                    .into_any_element()
                }
                None => div()
                    .truncate()
                    .text_color(colors.text_muted)
                    .child(row.destination.clone())
                    .into_any_element(),
            },
            "last" => match app.last_result() {
                Some((phase, at)) => h_flex()
                    .gap(u(5.0))
                    .text_size(u(12.0))
                    .text_color(colors.text_muted)
                    .child(widgets::result_icon(phase, colors).size(12.0))
                    .child(widgets::age_of(at))
                    .into_any_element(),
                None => div()
                    .text_color(colors.text_faint)
                    .child("—")
                    .into_any_element(),
            },
            _ => {
                let _ = cx;
                div().into_any_element()
            }
        }
    }
}

/// A chip with a count after it.
trait MapChip {
    fn map_chip(self, id: SharedString, count: usize, colors: &Colors) -> gpui::AnyElement;
}

impl MapChip for gpui::AnyElement {
    fn map_chip(self, id: SharedString, count: usize, colors: &Colors) -> gpui::AnyElement {
        h_flex()
            .id(id)
            .gap(u(4.0))
            .child(self)
            .child(
                div()
                    .font_family(fonts::MONO)
                    .text_size(u(11.0))
                    .text_color(colors.text_dim)
                    .child(count.to_string()),
            )
            .into_any_element()
    }
}

impl Focusable for AppsView {
    fn focus_handle(&self, _: &App) -> FocusHandle {
        self.focus.clone()
    }
}

impl TabView for AppsView {
    fn tab_title(&self, _: &App) -> SharedString {
        "Applications".into()
    }

    fn tab_icon(&self, _: &App) -> Option<SharedString> {
        Some(IconName::Layers.path())
    }

    fn tab_dot(&self, cx: &App) -> Option<gpui::Hsla> {
        // The cluster's color when it isn't the active one.
        let active = ActiveContext::global(cx)
            .cluster
            .as_ref()
            .map(|c| c.id.clone());
        (active.as_ref() != Some(&self.cluster))
            .then(|| ConnectionManager::try_global(cx).map(|m| m.read(cx).color(&self.cluster, cx)))
            .flatten()
    }

    fn view_request(&self, _: &App) -> Option<ViewRequest> {
        let gvr = self
            .gvr
            .clone()
            .unwrap_or_else(|| Gvr::new(crate::model::GROUP, "v1alpha1", "applications"));
        Some(ViewRequest::for_resource(
            ViewKind::Custom(VIEW_KIND.into()),
            ResourceRef::list(self.cluster.clone(), gvr, self.namespace.clone()),
        ))
    }
}

impl Render for AppsView {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let colors = cx.colors().clone();
        let toolbar = self.render_toolbar(window, cx);
        let chips = self.render_chips(cx);
        let header = self.render_header(cx);
        let status = self.status_label(cx);
        let body = if self.rows.is_empty() {
            let message = status.unwrap_or_else(|| {
                if self.all.is_empty() {
                    match &self.namespace {
                        Some(ns) => format!("No Applications in {ns}."),
                        None => "No Applications.".into(),
                    }
                } else {
                    "No Applications match the filters.".into()
                }
            });
            widgets::empty(message, &colors)
        } else {
            uniform_list(
                "argo-apps-rows",
                self.rows.len(),
                cx.processor(|this, range: Range<usize>, _, cx| this.render_rows(range, cx)),
            )
            .flex_1()
            .track_scroll(&self.scroll)
            .into_any_element()
        };
        let mut hints: Vec<(SharedString, SharedString)> = vec![("enter".into(), "Open".into())];
        hints.extend(ActionRegistry::global(cx).hints(APPS_CONTEXT));
        hints.push(("/".into(), "Filter".into()));
        v_flex()
            .key_context(APPS_CONTEXT)
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
                    .id("argo-apps-table")
                    .key_context(TABLE)
                    .track_focus(&self.focus)
                    .flex_1()
                    .min_h_0()
                    .on_action(cx.listener(|this, _: &nav::SelectNext, _, cx| {
                        this.move_selection(Move::Next, cx)
                    }))
                    .on_action(cx.listener(|this, _: &nav::SelectPrevious, _, cx| {
                        this.move_selection(Move::Previous, cx)
                    }))
                    .on_action(cx.listener(|this, _: &nav::SelectFirst, _, cx| {
                        this.move_selection(Move::First, cx)
                    }))
                    .on_action(cx.listener(|this, _: &nav::SelectLast, _, cx| {
                        this.move_selection(Move::Last, cx)
                    }))
                    .on_action(cx.listener(|this, _: &nav::SelectPageDown, _, cx| {
                        this.move_selection(Move::PageDown, cx)
                    }))
                    .on_action(cx.listener(|this, _: &nav::SelectPageUp, _, cx| {
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
