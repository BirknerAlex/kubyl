//! The Applications tab (board 23): a table of applications and a pane with the selected
//! one's objects.

use std::cell::{Cell, RefCell};
use std::collections::HashSet;
use std::rc::Rc;
use std::time::Duration;

use gpui::{
    App, AppContext as _, ClickEvent, Context, Entity, FocusHandle, Focusable, FontWeight,
    IntoElement, KeyBinding, Render, SharedString, Subscription, Task, Window, actions, div,
    prelude::*,
};
use gpui_component::button::{Button as MenuButton, ButtonVariants as _};
use gpui_component::input::{Input, InputEvent, InputState};
use gpui_component::menu::{DropdownMenu as _, PopupMenuItem};
use jiff::Timestamp;
use kubyl_apps_core::health::Health;
use kubyl_apps_core::kinds::Kind;
use kubyl_apps_core::model::{App as Application, Member};
use kubyl_apps_core::{build, table};
use kubyl_core::actions::{ExportCsv, OpenView};
use kubyl_core::{
    ActionRegistry, ActionSpec, ActiveContext, CellValue, ClusterId, ColumnDef, ColumnWidth, Gvr,
    Notification, NotificationCenter, ResourceRef, TabView, Tone, ViewKind, ViewRequest,
};
use kubyl_kube::{ConnectionEvent, ConnectionManager};
use kubyl_resources::store::find_resource;
use kubyl_resources::{ResourceStores, StoreHandle, StoreKey, StoreStatus};
use kubyl_ui::{
    ActiveColors, Chip, Colors, DataTable, DataTableEvent, Icon, IconName, KeyHints,
    SelectionScope, StatusPill, TableDelegate, fonts, h_flex, sizes, u, v_flex,
};

actions!(
    applications_view,
    [
        /// Focuses the filter field.
        FocusFilter,
        /// Streams the logs of the selected application's workload.
        ShowLogs,
        /// Opens the selected application's first workload.
        OpenFirstObject,
    ]
);

/// Key context of the tab.
pub const CONTEXT: &str = "ApplicationsView";

pub(crate) fn init(cx: &mut App) {
    cx.bind_keys([
        KeyBinding::new("/", FocusFilter, Some(CONTEXT)),
        KeyBinding::new("l", ShowLogs, Some(CONTEXT)),
    ]);
    ActionRegistry::register(
        cx,
        ActionSpec::new("Applications: Show Logs", ShowLogs)
            .hint("Logs")
            .in_context(CONTEXT),
    );
    ActionRegistry::register(
        cx,
        ActionSpec::new("Applications: Filter", FocusFilter)
            .hint("Filter")
            .in_context(CONTEXT),
    );
    ActionRegistry::register(
        cx,
        ActionSpec::new("Applications: Export CSV", ExportCsv).in_context(CONTEXT),
    );
}

/// The rows the table shows, shared with its delegate.
type Rows = Rc<RefCell<Vec<Application>>>;

struct Delegate {
    rows: Rows,
    now: Rc<Cell<Timestamp>>,
}

fn tone(health: Health) -> Tone {
    match health {
        Health::Degraded => Tone::Bad,
        Health::Progressing => Tone::Info,
        Health::Healthy => Tone::Good,
        Health::Suspended | Health::Unknown => Tone::Muted,
    }
}

impl TableDelegate for Delegate {
    fn columns(&self) -> Vec<ColumnDef> {
        let flex = |weight: f32, min: f32| ColumnWidth::Flex { weight, min };
        vec![
            ColumnDef::new("instance", "Instance", flex(1.2, 80.0)).mono(),
            ColumnDef::new("namespace", "Namespace", flex(0.8, 80.0)),
            ColumnDef::new("managed_by", "Managed by", flex(1.2, 90.0)),
            ColumnDef::new("version", "Version", ColumnWidth::Fixed(96.0)).mono(),
            ColumnDef::new("age", "Age", ColumnWidth::Fixed(48.0)).mono(),
            ColumnDef::new("status", "Status", ColumnWidth::Fixed(108.0)),
        ]
    }

    fn row_count(&self, _: &App) -> usize {
        self.rows.borrow().len()
    }

    fn cell(&self, row: usize, column: usize, _: &App) -> CellValue {
        let rows = self.rows.borrow();
        let Some(app) = rows.get(row) else {
            return CellValue::Empty;
        };
        let id = table::COLUMNS[column].0;
        let text = table::cell(app, id, self.now.get());
        match id {
            "status" => CellValue::Status {
                label: text.into(),
                tone: tone(app.health),
            },
            "managed_by" | "version" if text.is_empty() => CellValue::Empty,
            "namespace" => CellValue::Tinted {
                label: text.into(),
                tone: Tone::Neutral,
            },
            _ => CellValue::Text(text.into()),
        }
    }
}

/// The watch of a kind: only objects that carry the instance label (server-side), and for
/// everything but workloads only their metadata.
pub(crate) fn store_key(cluster: &ClusterId, kind: Kind, gvr: &Gvr) -> StoreKey {
    let key = StoreKey::new(cluster.clone(), gvr.clone(), None).labels(kubyl_apps_core::INSTANCE);
    if kind.needs_full_object() {
        key
    } else {
        key.metadata()
    }
}

/// What a watch of one kind is.
struct Source {
    kind: Kind,
    gvr: Gvr,
    store: StoreHandle,
}

pub struct AppsView {
    cluster: ClusterId,
    sources: Vec<Source>,
    /// The cluster's Argo CD Applications (their names tell label tracking apart from Helm's).
    argo: Option<StoreHandle>,
    all: Vec<Application>,
    rows: Rows,
    now: Rc<Cell<Timestamp>>,
    namespace: Option<String>,
    filter: String,
    filter_input: Entity<InputState>,
    table: Entity<DataTable>,
    /// `namespace/instance` of the selected row.
    selected: Option<String>,
    _observers: Vec<Subscription>,
    _subscriptions: Vec<Subscription>,
    _ticker: Task<()>,
}

impl AppsView {
    pub fn new(cluster: ClusterId, window: &mut Window, cx: &mut Context<Self>) -> Self {
        let rows: Rows = Rc::default();
        let now = Rc::new(Cell::new(Timestamp::now()));
        let table = cx.new(|cx| {
            DataTable::new(
                Delegate {
                    rows: rows.clone(),
                    now: now.clone(),
                },
                cx,
            )
        });
        let filter_input = cx.new(|cx| InputState::new(window, cx).placeholder("Filter"));
        let mut subscriptions = vec![
            cx.subscribe_in(
                &table,
                window,
                |this, _, event: &DataTableEvent, window, cx| this.table_event(event, window, cx),
            ),
            cx.subscribe_in(
                &filter_input,
                window,
                |this, input, event: &InputEvent, window, cx| match event {
                    InputEvent::Change => {
                        this.filter = input.read(cx).value().to_string();
                        this.refresh(cx);
                    }
                    InputEvent::PressEnter { .. } => {
                        let focus = this.table.read(cx).focus_handle(cx);
                        focus.focus(window, cx);
                    }
                    _ => {}
                },
            ),
        ];
        if let Some(manager) = ConnectionManager::try_global(cx) {
            let id = cluster.clone();
            subscriptions.push(cx.subscribe(
                &manager,
                move |this, _, event: &ConnectionEvent, cx| {
                    if matches!(event, ConnectionEvent::DiscoveryChanged(c) | ConnectionEvent::StateChanged(c) if *c == id)
                    {
                        this.sync_sources(cx);
                    }
                },
            ));
        }
        // Ages tick without events.
        let ticker = cx.spawn(async move |this, cx| {
            loop {
                cx.background_executor()
                    .timer(Duration::from_secs(30))
                    .await;
                if this.update(cx, |this, cx| this.refresh(cx)).is_err() {
                    break;
                }
            }
        });
        let mut this = Self {
            cluster,
            sources: Vec::new(),
            argo: None,
            all: Vec::new(),
            rows,
            now,
            namespace: None,
            filter: String::new(),
            filter_input,
            table,
            selected: None,
            _observers: Vec::new(),
            _subscriptions: subscriptions,
            _ticker: ticker,
        };
        this.sync_sources(cx);
        this
    }

    /// The served GVR of a kind (discovery's preferred version).
    fn gvr_of(&self, kind: Kind, cx: &App) -> Option<Gvr> {
        let discovery = ConnectionManager::try_global(cx)?
            .read(cx)
            .discovery(&self.cluster)?;
        find_resource(
            &discovery.resources,
            &Gvr::new(kind.group(), "", kind.plural()),
        )
        .map(|info| info.gvr.clone())
    }

    /// Watches the served kinds, only objects that carry the instance label (server-side), and
    /// the Argo CD Applications when the cluster has them.
    fn sync_sources(&mut self, cx: &mut Context<Self>) {
        let wanted: Vec<(Kind, Gvr)> = Kind::ALL
            .into_iter()
            .filter_map(|kind| Some((kind, self.gvr_of(kind, cx)?)))
            .collect();
        let argo_gvr = ConnectionManager::try_global(cx)
            .and_then(|m| m.read(cx).discovery(&self.cluster))
            .and_then(|d| {
                find_resource(
                    &d.resources,
                    &Gvr::new(kubyl_argocd_core::model::GROUP, "", "applications"),
                )
                .map(|i| i.gvr.clone())
            });
        self.set_sources(wanted, argo_gvr, cx);
    }

    /// Replaces the watches when the served kinds changed.
    pub(crate) fn set_sources(
        &mut self,
        wanted: Vec<(Kind, Gvr)>,
        argo_gvr: Option<Gvr>,
        cx: &mut Context<Self>,
    ) {
        let current: Vec<(Kind, Gvr)> = self
            .sources
            .iter()
            .map(|s| (s.kind, s.gvr.clone()))
            .collect();
        if current == wanted && self.argo.is_some() == argo_gvr.is_some() {
            return;
        }
        self.sources = wanted
            .into_iter()
            .map(|(kind, gvr)| {
                let key = store_key(&self.cluster, kind, &gvr);
                Source {
                    kind,
                    gvr,
                    store: ResourceStores::acquire(cx, key),
                }
            })
            .collect();
        self.argo = argo_gvr.map(|gvr| {
            ResourceStores::acquire(cx, kubyl_argocd_core::apps::all_key(&self.cluster, &gvr))
        });
        self._observers = self
            .sources
            .iter()
            .map(|s| s.store.entity().clone())
            .chain(self.argo.iter().map(|s| s.entity().clone()))
            .map(|store| cx.observe(&store, |this, _, cx| this.rebuild(cx)))
            .collect();
        self.rebuild(cx);
    }

    /// Groups the objects of every store again.
    fn rebuild(&mut self, cx: &mut Context<Self>) {
        let argo: HashSet<String> = self
            .argo
            .as_ref()
            .map(|store| {
                store
                    .read(cx)
                    .objects()
                    .values()
                    .filter_map(|o| {
                        o.pointer("/metadata/name")
                            .and_then(|n| n.as_str())
                            .map(str::to_string)
                    })
                    .collect()
            })
            .unwrap_or_default();
        let held: Vec<(Kind, std::sync::Arc<serde_json::Value>)> = self
            .sources
            .iter()
            .flat_map(|s| {
                s.store
                    .read(cx)
                    .objects()
                    .values()
                    .map(|o| (s.kind, o.clone()))
                    .collect::<Vec<_>>()
            })
            .collect();
        let objects: Vec<(Kind, &serde_json::Value)> =
            held.iter().map(|(k, o)| (*k, o.as_ref())).collect();
        self.all = build(&objects, &argo);
        self.refresh(cx);
    }

    /// The shown rows after a filter or the data changed, keeping the selection.
    fn refresh(&mut self, cx: &mut Context<Self>) {
        self.now.set(Timestamp::now());
        let shown: Vec<Application> = self
            .all
            .iter()
            .filter(|a| self.namespace.as_ref().is_none_or(|ns| *ns == a.namespace))
            .filter(|a| table::matches(a, &self.filter))
            .cloned()
            .collect();
        let keep = self
            .selected
            .as_ref()
            .and_then(|key| shown.iter().position(|a| a.key() == *key));
        *self.rows.borrow_mut() = shown;
        let first = (!self.rows.borrow().is_empty()).then_some(0);
        self.table.update(cx, |table, cx| {
            table.select(keep.or(first), cx);
        });
        // The table only tells when the selection moves.
        self.selected = self
            .table
            .read(cx)
            .selected()
            .and_then(|i| self.rows.borrow().get(i).map(Application::key));
        cx.notify();
    }

    fn table_event(&mut self, event: &DataTableEvent, window: &mut Window, cx: &mut Context<Self>) {
        match event {
            DataTableEvent::SelectionChanged(index) => {
                self.selected = index.and_then(|i| self.rows.borrow().get(i).map(Application::key));
                cx.notify();
            }
            DataTableEvent::Confirmed(_) => self.open_first(window, cx),
        }
    }

    pub fn selected_app(&self) -> Option<Application> {
        let key = self.selected.as_ref()?;
        self.rows.borrow().iter().find(|a| a.key() == *key).cloned()
    }

    /// The names of the applications shown, in order (tests).
    pub fn names(&self) -> Vec<String> {
        self.rows.borrow().iter().map(Application::key).collect()
    }

    fn object_ref(&self, member: &Member) -> ResourceRef {
        ResourceRef::object(
            self.cluster.clone(),
            Gvr::new(
                member.kind.group(),
                member.kind.version(),
                member.kind.plural(),
            ),
            Some(member.namespace.clone()),
            member.name.clone(),
        )
    }

    fn open_member(&self, member: &Member, window: &mut Window, cx: &mut App) {
        window.dispatch_action(
            Box::new(OpenView(ViewRequest::for_resource(
                ViewKind::Details,
                self.object_ref(member),
            ))),
            cx,
        );
    }

    fn open_logs(&self, member: &Member, window: &mut Window, cx: &mut App) {
        window.dispatch_action(
            Box::new(OpenView(ViewRequest::for_resource(
                ViewKind::Logs,
                self.object_ref(member),
            ))),
            cx,
        );
    }

    /// Enter: the application's first workload (else its first object).
    fn open_first(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if let Some(app) = self.selected_app()
            && let Some(member) = app.workloads().next().or(app.members.first())
        {
            self.open_member(member, window, cx);
        }
    }

    /// `l`: the logs of the application's workload; with several, a menu is in the pane.
    fn show_logs(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Some(app) = self.selected_app() else {
            return;
        };
        let sources: Vec<&Member> = app.log_sources().collect();
        match sources.as_slice() {
            [] => NotificationCenter::push(
                cx,
                Notification::info(format!(
                    "{} has no workload with logs (Deployment, StatefulSet, DaemonSet or Job).",
                    app.instance
                )),
            ),
            [one] => self.open_logs(one, window, cx),
            // Several: the first, the pane's Logs menu picks another.
            [first, ..] => self.open_logs(first, window, cx),
        }
    }

    /// The CSV of the table as shown (sort, filter and namespace applied) and the number of
    /// applications in it.
    pub fn csv_text(&self) -> Option<(String, usize)> {
        let rows = self.rows.borrow();
        if rows.is_empty() {
            return None;
        }
        let records = table::records(rows.iter(), Timestamp::now());
        Some((
            kubyl_core::csv::write(&table::header(), &records),
            records.len(),
        ))
    }

    /// "Export CSV": asks where to save the table.
    fn export_csv(&mut self, cx: &mut Context<Self>) {
        let Some((text, count)) = self.csv_text() else {
            NotificationCenter::push(
                cx,
                Notification::info("There are no applications to export."),
            );
            return;
        };
        let file = kubyl_core::export::file_name("applications", "csv", Timestamp::now());
        let what = format!("{count} application{}", if count == 1 { "" } else { "s" });
        kubyl_core::export::save_text(cx, &file, what, text);
    }

    /// Why nothing is listed, when a watch is missing or refused.
    fn problems(&self, cx: &App) -> Vec<String> {
        self.sources
            .iter()
            .filter_map(|s| kubyl_apps_core::model::problem(s.kind, s.store.read(cx).status()))
            .collect()
    }

    fn loading(&self, cx: &App) -> bool {
        self.sources.iter().any(|s| {
            matches!(
                s.store.read(cx).status(),
                StoreStatus::Waiting | StoreStatus::Loading
            )
        })
    }

    // ----- Rendering -----

    fn render_toolbar(
        &mut self,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> impl IntoElement + use<> {
        let colors = cx.colors().clone();
        let degraded = self
            .all
            .iter()
            .filter(|a| a.health == Health::Degraded)
            .count();
        let total = self.all.len();
        let shown = self.rows.borrow().len();
        let count = if shown == total {
            total.to_string()
        } else {
            format!("{shown} of {total}")
        };
        let crumb = h_flex()
            .flex_none()
            .gap(u(6.0))
            .text_color(colors.text_dim)
            .child(Icon::new(IconName::Blocks).color(colors.accent))
            .child(
                div()
                    .text_color(colors.text)
                    .font_weight(FontWeight::MEDIUM)
                    .child("Applications"),
            )
            .child("·")
            .child(count)
            .when(degraded > 0, |this| {
                this.child(
                    div()
                        .text_color(colors.red)
                        .child(format!("· {degraded} degraded")),
                )
            });

        let weak = cx.entity().downgrade();
        let mut namespaces: Vec<String> = self.all.iter().map(|a| a.namespace.clone()).collect();
        namespaces.sort();
        namespaces.dedup();
        let current = self.namespace.clone();
        let label = match &current {
            Some(ns) => format!("Namespace: {ns}"),
            None => "All namespaces".to_string(),
        };
        let namespace_menu = MenuButton::new("apps-namespace")
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
                    .child(label)
                    .child(Icon::new(IconName::ChevronDown).size(11.0)),
            )
            .dropdown_menu({
                let weak = weak.clone();
                move |menu, _, _| {
                    let mut menu = menu.max_h(gpui::px(420.0)).scrollable(true);
                    let all = weak.clone();
                    menu = menu.item(
                        PopupMenuItem::new("All namespaces")
                            .checked(current.is_none())
                            .on_click(move |_, _, cx| {
                                all.update(cx, |this, cx| {
                                    this.namespace = None;
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
                                .checked(current.as_deref() == Some(ns.as_str()))
                                .on_click(move |_, _, cx| {
                                    let value = value.clone();
                                    weak.update(cx, |this, cx| {
                                        this.namespace = Some(value);
                                        this.refresh(cx);
                                    })
                                    .ok();
                                }),
                        );
                    }
                    menu
                }
            });
        let export = {
            let weak = weak.clone();
            MenuButton::new("apps-menu")
                .ghost()
                .compact()
                .child(Icon::new(IconName::SlidersVertical).size(13.0))
                .dropdown_menu(move |menu, _, _| {
                    let weak = weak.clone();
                    menu.item(PopupMenuItem::new("Export CSV…").on_click(move |_, _, cx| {
                        weak.update(cx, |this, cx| this.export_csv(cx)).ok();
                    }))
                })
        };
        let focused = self
            .filter_input
            .read(cx)
            .focus_handle(cx)
            .is_focused(window);
        let filter = div()
            .flex_none()
            .w(u(180.0))
            .h(u(sizes::CONTROL))
            .px(u(8.0))
            .flex()
            .items_center()
            .gap(u(7.0))
            .rounded(u(5.0))
            .bg(colors.input_background)
            .border_1()
            .border_color(if focused {
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
        let (live, live_color) = if self.sources.is_empty() {
            ("…", colors.text_dim)
        } else if self.problems(cx).is_empty() {
            ("live", colors.green)
        } else {
            ("partial", colors.yellow)
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
            .child(namespace_menu)
            .child(filter)
            .child(export)
            .child(Chip::new(live).dot(live_color).text_color(live_color))
    }

    fn render_details(
        &self,
        app: &Application,
        cx: &mut Context<Self>,
    ) -> impl IntoElement + use<> {
        let colors = cx.colors().clone();
        let now = Timestamp::now();
        let kv = |key: &'static str, value: String| {
            h_flex()
                .gap(u(8.0))
                .items_start()
                .child(
                    div()
                        .flex_none()
                        .w(u(96.0))
                        .text_color(colors.text_dim)
                        .child(key),
                )
                .child(div().id(key).flex_1().min_w_0().child(if value.is_empty() {
                    "—".to_string()
                } else {
                    value
                }))
        };
        let section = |title: String| {
            div()
                .text_size(u(11.0))
                .font_weight(FontWeight::SEMIBOLD)
                .text_color(colors.text_dim)
                .child(title.to_uppercase())
        };
        let sources: Vec<Member> = app.log_sources().cloned().collect();
        let logs_button = (!sources.is_empty()).then(|| {
            let weak = cx.entity().downgrade();
            MenuButton::new("apps-logs")
                .compact()
                .child(
                    h_flex()
                        .gap(u(5.0))
                        .text_size(u(12.0))
                        .child(Icon::new(IconName::List).size(12.0))
                        .child("Logs")
                        .when(sources.len() > 1, |this| {
                            this.child(Icon::new(IconName::ChevronDown).size(10.0))
                        }),
                )
                .dropdown_menu(move |menu, _, _| {
                    let mut menu = menu;
                    for member in &sources {
                        let weak = weak.clone();
                        let member = member.clone();
                        menu = menu.item(
                            PopupMenuItem::new(format!("{} {}", member.kind.kind(), member.name))
                                .on_click(move |_, window, cx| {
                                    let member = member.clone();
                                    weak.update(cx, |this, cx| this.open_logs(&member, window, cx))
                                        .ok();
                                }),
                        );
                    }
                    menu
                })
        });
        let mut members = v_flex().gap(u(2.0));
        let mut last_kind = None;
        for (ix, member) in app.members.iter().enumerate() {
            if last_kind != Some(member.kind) {
                last_kind = Some(member.kind);
                members = members.child(
                    div()
                        .pt(u(6.0))
                        .text_size(u(11.5))
                        .text_color(colors.text_dim)
                        .child(member.kind.kind()),
                );
            }
            let clicked = member.clone();
            let weak = cx.entity().downgrade();
            members = members.child(
                h_flex()
                    .id(("apps-member", ix))
                    .gap(u(8.0))
                    .h(u(24.0))
                    .child(
                        div()
                            .id(("apps-member-link", ix))
                            .flex_1()
                            .min_w_0()
                            .truncate()
                            .font_family(fonts::MONO)
                            .text_size(u(12.0))
                            .text_color(colors.accent)
                            .cursor_pointer()
                            .hover(|s| s.underline())
                            .on_click(move |_: &ClickEvent, window, cx| {
                                let member = clicked.clone();
                                weak.update(cx, |this, cx| this.open_member(&member, window, cx))
                                    .ok();
                            })
                            .child(member.name.clone()),
                    )
                    .when_some(member.health, |this, health| {
                        this.child(
                            div()
                                .text_size(u(11.5))
                                .text_color(colors.text_dim)
                                .child(member.detail.clone()),
                        )
                        .child(StatusPill::new(health.label(), tone(health)))
                    }),
            );
        }
        let names = app.names.iter().cloned().collect::<Vec<_>>().join(", ");
        v_flex()
            .id("apps-details")
            .debug_selector(|| "apps-details".into())
            .flex_none()
            .w(u(310.0))
            .h_full()
            .overflow_y_scroll()
            .bg(colors.panel)
            .border_l_1()
            .border_color(colors.border)
            .child(
                v_flex()
                    .px(u(14.0))
                    .py(u(12.0))
                    .gap(u(8.0))
                    .border_b_1()
                    .border_color(colors.border_variant)
                    .child(
                        div()
                            .font_family(fonts::MONO)
                            .text_size(u(12.5))
                            .child(app.instance.clone()),
                    )
                    .child(
                        h_flex()
                            .gap(u(6.0))
                            .child(StatusPill::new(app.health.label(), tone(app.health)))
                            .child(Chip::new(app.namespace.clone()))
                            .children(
                                (!app.manager.tool().is_empty())
                                    .then(|| Chip::new(app.manager.tool().to_string())),
                            )
                            .child(div().flex_1())
                            .children(logs_button),
                    ),
            )
            .child(
                v_flex()
                    .px(u(14.0))
                    .py(u(12.0))
                    .gap(u(6.0))
                    .text_size(u(12.0))
                    .border_b_1()
                    .border_color(colors.border_variant)
                    .child(section("Application".into()))
                    .child(kv("Managed by", app.manager.label()))
                    .child(kv("Version", app.version.clone()))
                    .child(kv("Name", names))
                    .child(kv("Part of", app.part_of.clone().unwrap_or_default()))
                    .child(kv("Age", table::age(app, now)))
                    .child(kv("Selector", app.selector())),
            )
            .child(
                v_flex()
                    .px(u(14.0))
                    .py(u(12.0))
                    .gap(u(6.0))
                    .child(section(format!("Objects · {}", app.members.len())))
                    .child(members),
            )
    }

    fn empty_message(&self, cx: &App) -> Option<String> {
        if !self.rows.borrow().is_empty() {
            return None;
        }
        let problems = self.problems(cx);
        if self.sources.is_empty() {
            return Some("Not connected to the cluster yet.".into());
        }
        if self.loading(cx) && self.all.is_empty() {
            return Some("Loading applications…".into());
        }
        if !problems.is_empty() && self.all.is_empty() {
            return Some(problems.join(" "));
        }
        Some(if self.all.is_empty() {
            "No application found. Objects join one through the app.kubernetes.io/instance label \
             (Helm sets it; so do most charts, Argo CD and Kustomize setups)."
                .into()
        } else {
            "No application matches the filter.".into()
        })
    }
}

impl Focusable for AppsView {
    /// The table, so arrow keys and `j`/`k` work right away.
    fn focus_handle(&self, cx: &App) -> FocusHandle {
        self.table.read(cx).focus_handle(cx)
    }
}

impl TabView for AppsView {
    fn tab_title(&self, _: &App) -> SharedString {
        "Applications".into()
    }

    fn tab_icon(&self, _: &App) -> Option<SharedString> {
        Some(IconName::Blocks.path())
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
            crate::target(&self.cluster),
        ))
    }
}

fn banner(text: String, colors: &Colors) -> impl IntoElement {
    h_flex()
        .flex_none()
        .px(u(12.0))
        .py(u(6.0))
        .gap(u(8.0))
        .bg(colors.yellow.opacity(0.1))
        .border_b_1()
        .border_color(colors.yellow.opacity(0.35))
        .text_size(u(12.0))
        .child(
            Icon::new(IconName::TriangleAlert)
                .size(12.0)
                .color(colors.yellow),
        )
        .child(text)
}

impl Render for AppsView {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let colors = cx.colors().clone();
        let toolbar = self.render_toolbar(window, cx);
        let problems = self.problems(cx);
        let details = self.selected_app().map(|app| self.render_details(&app, cx));
        let body = match self.empty_message(cx) {
            Some(message) => div()
                .debug_selector(|| "apps-empty".into())
                .flex_1()
                .flex()
                .items_center()
                .justify_center()
                .p(u(24.0))
                .text_color(colors.text_dim)
                .text_size(u(12.5))
                .child(message)
                .into_any_element(),
            None => div()
                .flex_1()
                .min_h_0()
                .flex()
                .debug_selector(|| "apps-table".into())
                .child(self.table.clone())
                .into_any_element(),
        };
        let hints = vec![
            ("↵".into(), "Open".into()),
            ("l".into(), "Logs".into()),
            ("/".into(), "Filter".into()),
        ];
        v_flex()
            .key_context(CONTEXT)
            .size_full()
            .bg(colors.background)
            .text_color(colors.text)
            .font_family(fonts::UI)
            .on_action(cx.listener(|this, _: &FocusFilter, window, cx| {
                let focus = this.filter_input.read(cx).focus_handle(cx);
                focus.focus(window, cx);
            }))
            .on_action(cx.listener(|this, _: &ShowLogs, window, cx| this.show_logs(window, cx)))
            .on_action(
                cx.listener(|this, _: &OpenFirstObject, window, cx| this.open_first(window, cx)),
            )
            .on_action(cx.listener(|this, _: &ExportCsv, _, cx| this.export_csv(cx)))
            .child(toolbar)
            .children(
                (!problems.is_empty() && !self.all.is_empty())
                    .then(|| banner(problems.join(" "), &colors)),
            )
            .child(
                h_flex()
                    .flex_1()
                    .min_h_0()
                    .child(v_flex().flex_1().min_w_0().h_full().child(body))
                    .children(details.map(|details| {
                        SelectionScope::new(("apps-details", cx.entity_id().as_u64()), details)
                    })),
            )
            .child(KeyHints::new(hints))
    }
}

#[cfg(test)]
mod tests {
    use std::cell::RefCell;
    use std::rc::Rc;

    use gpui::TestAppContext;
    use kubyl_resources::{ResourceStore, ResourceStores};
    use serde_json::{Value, json};

    use super::*;

    const INSTANCE: &str = kubyl_apps_core::INSTANCE;

    fn deployment(name: &str, ns: &str, instance: &str, ready: i64) -> Value {
        json!({"metadata": {"name": name, "namespace": ns, "creationTimestamp": "2026-10-01T10:00:00Z",
                "labels": {INSTANCE: instance, "app.kubernetes.io/version": "1.4.2",
                    "app.kubernetes.io/managed-by": "Helm"},
                "annotations": {"meta.helm.sh/release-name": instance, "meta.helm.sh/release-namespace": ns}},
            "spec": {"replicas": 2},
            "status": {"replicas": 2, "readyReplicas": ready, "updatedReplicas": 2}})
    }

    fn service(name: &str, ns: &str, instance: &str) -> Value {
        json!({"metadata": {"name": name, "namespace": ns, "labels": {INSTANCE: instance}}})
    }

    /// Fills the stores a view watches and builds the view in a window.
    fn setup(
        cx: &mut TestAppContext,
        objects: Vec<(Kind, Vec<Value>)>,
    ) -> (Entity<AppsView>, &mut gpui::VisualTestContext) {
        let dir = tempfile::tempdir().unwrap();
        let cluster = ClusterId::new("kind-apps@/k");
        let wanted: Vec<(Kind, Gvr)> = Kind::ALL
            .into_iter()
            .map(|k| (k, Gvr::new(k.group(), k.version(), k.plural())))
            .collect();
        let keep = cx.update(|cx| {
            kubyl_core::init(cx);
            kubyl_settings::init_with_dir(cx, dir.path());
            kubyl_ui::init(cx);
            let mut handles = Vec::new();
            for (kind, gvr) in &wanted {
                let key = store_key(&cluster, *kind, gvr);
                let values = objects
                    .iter()
                    .filter(|(k, _)| k == kind)
                    .flat_map(|(_, v)| v.clone())
                    .collect::<Vec<_>>();
                let store = cx.new(|_| ResourceStore::from_objects(key.clone(), values));
                handles.push(ResourceStores::insert(cx, key, store));
            }
            handles
        });
        // The directory and the handles live as long as the test's app.
        Box::leak(Box::new((dir, keep)));
        let slot: Rc<RefCell<Option<Entity<AppsView>>>> = Rc::default();
        let (_root, cx) = cx.add_window_view({
            let slot = slot.clone();
            move |window, cx| {
                let view = cx.new(|cx| {
                    let mut view = AppsView::new(cluster, window, cx);
                    view.set_sources(wanted, None, cx);
                    view
                });
                *slot.borrow_mut() = Some(view.clone());
                gpui_component::Root::new(view, window, cx)
            }
        });
        cx.run_until_parked();
        let view = slot.borrow().clone().unwrap();
        (view, cx)
    }

    #[gpui::test]
    fn applications_are_grouped_sorted_and_selected(cx: &mut TestAppContext) {
        let (view, cx) = setup(
            cx,
            vec![
                (
                    Kind::Deployment,
                    vec![
                        deployment("shop-web", "shop", "shop", 2),
                        deployment("ledger", "bank", "ledger", 1),
                        // Without the label: no application (the server filters it out; the
                        // grouping skips it too).
                        json!({"metadata": {"name": "tool", "namespace": "shop"}}),
                    ],
                ),
                (Kind::Service, vec![service("shop-web", "shop", "shop")]),
            ],
        );
        view.update(cx, |view, _| {
            // Progressing (1/2 ready) comes before healthy.
            assert_eq!(view.names(), ["bank/ledger", "shop/shop"]);
            assert_eq!(view.selected_app().unwrap().key(), "bank/ledger");
        });
        assert!(cx.debug_bounds("apps-table").is_some());
        assert!(cx.debug_bounds("apps-details").is_some());
        assert!(cx.debug_bounds("apps-empty").is_none());
        // Members of the selected application are listed with links.
        view.update(cx, |view, _| {
            let app = view.selected_app().unwrap();
            assert_eq!(app.members.len(), 1);
            assert_eq!(app.version, "1.4.2");
            assert_eq!(app.manager.label(), "Helm · ledger");
        });
    }

    #[gpui::test]
    fn filters_and_namespaces_narrow_the_table_and_the_csv(cx: &mut TestAppContext) {
        let (view, cx) = setup(
            cx,
            vec![(
                Kind::Deployment,
                vec![
                    deployment("a", "shop", "shop", 2),
                    deployment("b", "shop", "cart", 2),
                    deployment("c", "bank", "ledger", 2),
                ],
            )],
        );
        view.update(cx, |view, cx| {
            assert_eq!(view.names(), ["bank/ledger", "shop/cart", "shop/shop"]);
            view.filter = "cart".into();
            view.refresh(cx);
            assert_eq!(view.names(), ["shop/cart"]);
            // The CSV is what the table shows, header first, CRLF.
            let (csv, count) = view.csv_text().unwrap();
            assert_eq!(count, 1);
            let lines: Vec<&str> = csv.split("\r\n").collect();
            assert_eq!(lines[0], "Instance,Namespace,Managed by,Version,Age,Status");
            assert!(
                lines[1].starts_with("cart,shop,Helm · cart,1.4.2,"),
                "{csv}"
            );
            assert!(lines[1].ends_with(",Healthy"));
            view.filter.clear();
            view.namespace = Some("bank".into());
            view.refresh(cx);
            assert_eq!(view.names(), ["bank/ledger"]);
            view.filter = "nothing".into();
            view.refresh(cx);
            assert!(view.names().is_empty());
            assert!(view.csv_text().is_none());
            assert_eq!(
                view.empty_message(cx).as_deref(),
                Some("No application matches the filter.")
            );
        });
        cx.run_until_parked();
        assert!(cx.debug_bounds("apps-empty").is_some());
        assert!(cx.debug_bounds("apps-details").is_none());
    }

    #[gpui::test]
    fn an_empty_cluster_explains_how_objects_join_an_application(cx: &mut TestAppContext) {
        let (view, cx) = setup(cx, vec![]);
        view.update(cx, |view, cx| {
            let message = view.empty_message(cx).unwrap();
            assert!(message.contains("app.kubernetes.io/instance"), "{message}");
        });
        assert!(cx.debug_bounds("apps-empty").is_some());
    }

    #[gpui::test]
    fn instance_labels_cannot_inject_formulas_into_the_csv(cx: &mut TestAppContext) {
        let (view, cx) = setup(
            cx,
            vec![(
                Kind::Deployment,
                vec![deployment("x", "shop", "=HYPERLINK(\"http://evil\")", 2)],
            )],
        );
        view.update(cx, |view, _| {
            let (csv, _) = view.csv_text().unwrap();
            assert!(
                csv.lines().nth(1).unwrap().starts_with("\"'=HYPERLINK("),
                "{csv}"
            );
        });
    }
}
