//! The Security Center tab (board 23): sub-tabs Images · Resources · Roles, a severity
//! summary, a table and the findings of the selected row.

use std::cell::{Cell, RefCell};
use std::rc::Rc;
use std::time::Duration;

use gpui::{
    App, AppContext as _, Context, Entity, FocusHandle, Focusable, FontWeight, IntoElement,
    KeyBinding, Render, SharedString, Subscription, Task, Window, actions, div, prelude::*,
};
use gpui_component::button::{Button as MenuButton, ButtonVariants as _};
use gpui_component::input::{Input, InputEvent, InputState};
use gpui_component::menu::{DropdownMenu as _, PopupMenuItem};
use jiff::Timestamp;
use kubyl_core::actions::ExportCsv;
use kubyl_core::{
    ActionRegistry, ActionSpec, ActiveContext, CellValue, ClusterId, ColumnDef, ColumnWidth,
    Notification, NotificationCenter, TabView, Tone, ViewKind, ViewRequest,
};
use kubyl_helm::dialogs::{InstallRequest, open_install};
use kubyl_helm_core::repo::ChartRef;
use kubyl_kube::{ConnectionEvent, ConnectionManager};
use kubyl_security_core::aggregate::{self, Filter, ImageRow, ReportRef, ResourceRow, RoleRow};
use kubyl_security_core::details::{self, Details, SHOWN};
use kubyl_security_core::kinds::{GROUP, View};
use kubyl_security_core::model::{Counts, Severity};
use kubyl_security_core::{install, table};
use kubyl_ui::{
    ActiveColors, Button, Chip, Colors, DataTable, DataTableEvent, Icon, IconName, KeyHints,
    SelectionScope, StatusPill, TableDelegate, fonts, h_flex, sizes, u, v_flex,
};
use serde_json::Value;

use crate::feed::Feed;

actions!(
    security_view,
    [
        /// Focuses the filter field.
        FocusFilter,
        /// Shows the Images view.
        ShowImages,
        /// Shows the Resources view.
        ShowResources,
        /// Shows the Roles view.
        ShowRoles,
    ]
);

/// Key context of the tab.
pub const CONTEXT: &str = "SecurityView";

pub(crate) fn init(cx: &mut App) {
    cx.bind_keys([
        KeyBinding::new("/", FocusFilter, Some(CONTEXT)),
        KeyBinding::new("1", ShowImages, Some(CONTEXT)),
        KeyBinding::new("2", ShowResources, Some(CONTEXT)),
        KeyBinding::new("3", ShowRoles, Some(CONTEXT)),
    ]);
    ActionRegistry::register(
        cx,
        ActionSpec::new("Security: Filter", FocusFilter)
            .hint("Filter")
            .in_context(CONTEXT),
    );
    ActionRegistry::register(
        cx,
        ActionSpec::new("Security: Export CSV", ExportCsv).in_context(CONTEXT),
    );
    ActionRegistry::register(
        cx,
        ActionSpec::new("Security: Show Images", ShowImages).in_context(CONTEXT),
    );
    ActionRegistry::register(
        cx,
        ActionSpec::new("Security: Show Resources", ShowResources).in_context(CONTEXT),
    );
    ActionRegistry::register(
        cx,
        ActionSpec::new("Security: Show Roles", ShowRoles).in_context(CONTEXT),
    );
}

/// The rows of the shown view.
#[derive(Clone, Debug, Default)]
pub enum Rows {
    #[default]
    None,
    Images(Vec<ImageRow>),
    Resources(Vec<ResourceRow>),
    Roles(Vec<RoleRow>),
}

impl Rows {
    fn len(&self) -> usize {
        match self {
            Rows::None => 0,
            Rows::Images(r) => r.len(),
            Rows::Resources(r) => r.len(),
            Rows::Roles(r) => r.len(),
        }
    }

    fn counts(&self) -> Vec<Counts> {
        match self {
            Rows::None => Vec::new(),
            Rows::Images(r) => r.iter().map(|r| r.counts).collect(),
            Rows::Resources(r) => r.iter().map(|r| r.counts).collect(),
            Rows::Roles(r) => r.iter().map(|r| r.counts).collect(),
        }
    }

    /// The CSV records, in table order.
    fn records(&self, now: Timestamp) -> Vec<Vec<String>> {
        match self {
            Rows::None => Vec::new(),
            Rows::Images(r) => table::image_records(r, now),
            Rows::Resources(r) => table::resource_records(r, now),
            Rows::Roles(r) => table::role_records(r, now),
        }
    }
}

struct Delegate {
    view: View,
    rows: Rc<RefCell<Rows>>,
    now: Rc<Cell<Timestamp>>,
}

fn tone_of(severity: &str, count: u32) -> Tone {
    if count == 0 {
        return Tone::Muted;
    }
    match severity {
        "critical" => Tone::Bad,
        "high" => Tone::Warning,
        "medium" => Tone::Info,
        _ => Tone::Neutral,
    }
}

impl TableDelegate for Delegate {
    fn columns(&self) -> Vec<ColumnDef> {
        let flex = |weight: f32, min: f32| ColumnWidth::Flex { weight, min };
        table::columns(self.view)
            .into_iter()
            .map(|(id, title)| match id {
                "image" | "resource" | "role" => ColumnDef::new(id, title, flex(2.0, 200.0)).mono(),
                "namespace" | "scanner" => ColumnDef::new(id, title, ColumnWidth::Fixed(110.0)),
                "workloads" => ColumnDef::new(id, title, ColumnWidth::Fixed(84.0))
                    .align_end()
                    .mono(),
                "age" => ColumnDef::new(id, title, ColumnWidth::Fixed(52.0)).mono(),
                _ => ColumnDef::new(id, title, ColumnWidth::Fixed(66.0))
                    .align_end()
                    .mono(),
            })
            .collect()
    }

    fn row_count(&self, _: &App) -> usize {
        self.rows.borrow().len()
    }

    fn cell(&self, row: usize, column: usize, _: &App) -> CellValue {
        let id = table::columns(self.view)[column].0;
        let now = self.now.get();
        let (text, counts) = match &*self.rows.borrow() {
            Rows::None => return CellValue::Empty,
            Rows::Images(rows) => match rows.get(row) {
                Some(r) => (table::image_cell(r, id, now), Some(r.counts)),
                None => return CellValue::Empty,
            },
            Rows::Resources(rows) => match rows.get(row) {
                Some(r) => (table::resource_cell(r, id, now), Some(r.counts)),
                None => return CellValue::Empty,
            },
            Rows::Roles(rows) => match rows.get(row) {
                Some(r) => (table::role_cell(r, id, now), Some(r.counts)),
                None => return CellValue::Empty,
            },
        };
        if text.is_empty() {
            return CellValue::Empty;
        }
        match id {
            "critical" | "high" | "medium" | "low" => {
                let count = counts.map_or(0, |c| match id {
                    "critical" => c.critical,
                    "high" => c.high,
                    "medium" => c.medium,
                    _ => c.low,
                });
                CellValue::Tinted {
                    label: text.into(),
                    tone: tone_of(id, count),
                }
            }
            "secrets" => CellValue::Tinted {
                label: text.clone().into(),
                tone: if text == "0" { Tone::Muted } else { Tone::Bad },
            },
            "namespace" => CellValue::Tinted {
                label: text.into(),
                tone: Tone::Neutral,
            },
            _ => CellValue::Text(text.into()),
        }
    }
}

/// The full report of the selected row.
enum Findings {
    Loading(#[allow(dead_code)] Task<()>),
    Ready(Details),
    Failed(String),
}

pub struct SecurityView {
    cluster: ClusterId,
    view: View,
    feed: Entity<Feed>,
    rows: Rc<RefCell<Rows>>,
    now: Rc<Cell<Timestamp>>,
    table: Entity<DataTable>,
    filter: Filter,
    filter_input: Entity<InputState>,
    /// The row key (image, `Kind name`) whose findings are shown.
    selected: Option<String>,
    findings: Option<(String, Findings)>,
    /// Critical findings per view (Images, Resources, Roles), shown on the sub-tabs so the
    /// sidebar badge (their sum) can be told apart.
    view_criticals: [u32; 3],
    focus: FocusHandle,
    /// Tests have no discovery: pretend the cluster serves no report kind.
    #[cfg(test)]
    assume_missing: bool,
    _subscriptions: Vec<Subscription>,
    _ticker: Task<()>,
}

impl SecurityView {
    pub fn new(
        cluster: ClusterId,
        view: View,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Self {
        let feed = cx.new(|_| Feed::new(cluster.clone()));
        let rows = Rc::new(RefCell::new(Rows::None));
        let now = Rc::new(Cell::new(Timestamp::now()));
        let table = Self::new_table(view, &rows, &now, cx);
        let filter_input = cx.new(|cx| InputState::new(window, cx).placeholder("Filter"));
        let mut subscriptions = vec![
            cx.observe(&feed, |this, _, cx| this.rebuild(cx)),
            cx.subscribe_in(
                &filter_input,
                window,
                |this, input, event: &InputEvent, window, cx| match event {
                    InputEvent::Change => {
                        this.filter.text = input.read(cx).value().to_string();
                        this.rebuild(cx);
                    }
                    InputEvent::PressEnter { .. } => {
                        let focus = this.table.read(cx).focus_handle(cx);
                        focus.focus(window, cx);
                    }
                    _ => {}
                },
            ),
        ];
        subscriptions.push(cx.subscribe_in(
            &table,
            window,
            |this, _, event: &DataTableEvent, _, cx| this.table_event(event, cx),
        ));
        if let Some(manager) = ConnectionManager::try_global(cx) {
            let id = cluster.clone();
            subscriptions.push(cx.subscribe(&manager, move |this, _, event: &ConnectionEvent, cx| {
                if matches!(event, ConnectionEvent::DiscoveryChanged(c) | ConnectionEvent::StateChanged(c) if *c == id)
                {
                    this.feed.update(cx, |feed, cx| feed.sync(cx));
                    cx.notify();
                }
            }));
        }
        let ticker = cx.spawn(async move |this, cx| {
            loop {
                cx.background_executor()
                    .timer(Duration::from_secs(30))
                    .await;
                if this.update(cx, |this, cx| this.rebuild(cx)).is_err() {
                    break;
                }
            }
        });
        let this = Self {
            cluster,
            view,
            feed,
            rows,
            now,
            table,
            filter: Filter::default(),
            filter_input,
            selected: None,
            findings: None,
            view_criticals: [0; 3],
            focus: cx.focus_handle(),
            #[cfg(test)]
            assume_missing: false,
            _subscriptions: subscriptions,
            _ticker: ticker,
        };
        this.feed.update(cx, |feed, cx| feed.sync(cx));
        this
    }

    fn new_table(
        view: View,
        rows: &Rc<RefCell<Rows>>,
        now: &Rc<Cell<Timestamp>>,
        cx: &mut Context<Self>,
    ) -> Entity<DataTable> {
        cx.new(|cx| {
            DataTable::new(
                Delegate {
                    view,
                    rows: rows.clone(),
                    now: now.clone(),
                },
                cx,
            )
        })
    }

    pub fn feed(&self) -> &Entity<Feed> {
        &self.feed
    }

    pub fn view(&self) -> View {
        self.view
    }

    fn caps(&self, cx: &App) -> kubyl_core::TrivyCaps {
        ConnectionManager::try_global(cx)
            .map(|m| m.read(cx).caps(&self.cluster).trivy)
            .unwrap_or_default()
    }

    /// Whether the cluster serves no Trivy report at all (the install empty state).
    fn missing(&self, cx: &App) -> bool {
        #[cfg(test)]
        if self.assume_missing {
            return true;
        }
        !self.caps(cx).any() && self.discovered(cx)
    }

    fn discovered(&self, cx: &App) -> bool {
        ConnectionManager::try_global(cx)
            .is_some_and(|m| m.read(cx).discovery(&self.cluster).is_some())
    }

    /// Shows another view; its table is built anew (its columns differ).
    pub fn show(&mut self, view: View, window: &mut Window, cx: &mut Context<Self>) {
        if view == self.view {
            return;
        }
        self.view = view;
        self.selected = None;
        self.findings = None;
        self.table = Self::new_table(view, &self.rows, &self.now, cx);
        self._subscriptions.push(cx.subscribe_in(
            &self.table.clone(),
            window,
            |this, _, event: &DataTableEvent, _, cx| this.table_event(event, cx),
        ));
        self.rebuild(cx);
    }

    /// Groups the reports of the shown view again, after a change of reports or filter.
    fn rebuild(&mut self, cx: &mut Context<Self>) {
        self.now.set(Timestamp::now());
        let reports = self.feed.read(cx).reports(cx);
        let summary = aggregate::summary(&reports, None);
        self.view_criticals = [
            summary.images.critical,
            summary.resources.critical,
            summary.roles.critical,
        ];
        let rows = match self.view {
            View::Images => Rows::Images(
                aggregate::images(&reports)
                    .into_iter()
                    .filter(|r| r.matches(&self.filter))
                    .collect(),
            ),
            View::Resources => Rows::Resources(
                aggregate::resources(&reports)
                    .into_iter()
                    .filter(|r| r.matches(&self.filter))
                    .collect(),
            ),
            View::Roles => Rows::Roles(
                aggregate::roles(&reports)
                    .into_iter()
                    .filter(|r| r.matches(&self.filter))
                    .collect(),
            ),
        };
        let keep = self
            .selected
            .as_ref()
            .and_then(|key| row_keys(&rows).iter().position(|k| k == key));
        let first = (rows.len() > 0).then_some(0);
        *self.rows.borrow_mut() = rows;
        self.table
            .update(cx, |table, cx| table.select(keep.or(first), cx));
        let selected = self
            .table
            .read(cx)
            .selected()
            .and_then(|i| row_keys(&self.rows.borrow()).get(i).cloned());
        if selected != self.selected {
            self.selected = selected;
            self.load_findings(cx);
        }
        cx.notify();
    }

    fn table_event(&mut self, event: &DataTableEvent, cx: &mut Context<Self>) {
        if let DataTableEvent::SelectionChanged(index) = event {
            self.selected = index.and_then(|i| row_keys(&self.rows.borrow()).get(i).cloned());
            self.load_findings(cx);
            cx.notify();
        }
    }

    /// The reports behind the selected row.
    fn selected_reports(&self) -> Vec<ReportRef> {
        let Some(key) = &self.selected else {
            return Vec::new();
        };
        match &*self.rows.borrow() {
            Rows::None => Vec::new(),
            Rows::Images(rows) => rows
                .iter()
                .find(|r| r.image == *key)
                .map(|r| {
                    // One scan of the image is enough: the same findings.
                    r.vulnerability_reports
                        .iter()
                        .take(1)
                        .chain(r.secret_reports.iter().take(1))
                        .cloned()
                        .collect()
                })
                .unwrap_or_default(),
            Rows::Resources(rows) => rows
                .iter()
                .find(|r| r.subject.label() == *key)
                .map(|r| {
                    r.config_report
                        .iter()
                        .chain(r.secret_reports.iter())
                        .cloned()
                        .collect()
                })
                .unwrap_or_default(),
            Rows::Roles(rows) => rows
                .iter()
                .find(|r| r.subject.label() == *key)
                .map(|r| vec![r.report.clone()])
                .unwrap_or_default(),
        }
    }

    /// Fetches the selected row's full reports on Tokio (one object each, never a list).
    fn load_findings(&mut self, cx: &mut Context<Self>) {
        let Some(key) = self.selected.clone() else {
            self.findings = None;
            return;
        };
        let reports = self.selected_reports();
        let Some(client) =
            ConnectionManager::try_global(cx).and_then(|m| m.read(cx).client(&self.cluster))
        else {
            return;
        };
        let fetch = kubyl_core::spawn_kube(cx, async move {
            let mut all = Details::default();
            for report in reports {
                let object = fetch_report(&client, &report).await?;
                let parsed = details::parse(report.kind, &object);
                if all.artifact.is_empty() {
                    all.artifact = parsed.artifact;
                    all.os = parsed.os;
                }
                all.vulnerabilities.extend(parsed.vulnerabilities);
                all.checks.extend(parsed.checks);
                all.secrets.extend(parsed.secrets);
            }
            Ok::<Details, String>(all)
        });
        let task_key = key.clone();
        let task = cx.spawn(async move |this, cx| {
            let result = fetch.await;
            this.update(cx, |this, cx| {
                if this.selected.as_ref() != Some(&task_key) {
                    return;
                }
                this.findings = Some((
                    task_key,
                    match result {
                        Ok(details) => Findings::Ready(details),
                        Err(err) => Findings::Failed(err),
                    },
                ));
                cx.notify();
            })
            .ok();
        });
        self.findings = Some((key, Findings::Loading(task)));
    }

    /// The findings shown for a row, set directly (tests).
    #[cfg(test)]
    pub(crate) fn set_findings(&mut self, key: &str, details: Details) {
        self.findings = Some((key.to_string(), Findings::Ready(details)));
    }

    /// The CSV of the table as shown, and how many rows it has.
    pub fn csv_text(&self) -> Option<(String, usize)> {
        let rows = self.rows.borrow();
        if rows.len() == 0 {
            return None;
        }
        let records = rows.records(Timestamp::now());
        Some((
            kubyl_core::csv::write(&table::header(self.view), &records),
            records.len(),
        ))
    }

    fn export_csv(&mut self, cx: &mut Context<Self>) {
        let Some((text, count)) = self.csv_text() else {
            NotificationCenter::push(cx, Notification::info("There is nothing to export."));
            return;
        };
        let file = kubyl_core::export::file_name(
            &format!("security-{}", self.view.id()),
            "csv",
            Timestamp::now(),
        );
        kubyl_core::export::save_text(cx, &file, format!("{count} rows"), text);
    }

    /// The findings of the selected row as CSV (all of them, not the panel's cut).
    fn export_findings(&mut self, cx: &mut Context<Self>) {
        let Some((key, Findings::Ready(details))) = &self.findings else {
            return;
        };
        let text = table::details_csv(details);
        let file = kubyl_core::export::file_name(
            &format!("security-{}", key.replace(['/', ' ', ':'], "-")),
            "csv",
            Timestamp::now(),
        );
        let what = format!("{} findings", details.len());
        kubyl_core::export::save_text(cx, &file, what, text);
    }

    fn install(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        // The Helm flow guards read-only clusters and asks for the typed cluster name on PROD.
        open_install(
            InstallRequest {
                cluster: Some(self.cluster.clone()),
                chart: Some(ChartRef::Url {
                    repo_url: install::CHART_REPO.into(),
                    name: install::CHART.into(),
                }),
                version: None,
                namespace: Some(install::NAMESPACE.into()),
            },
            window,
            cx,
        );
    }

    // ----- Rendering -----

    fn render_tabs(&self, cx: &mut Context<Self>) -> impl IntoElement + use<> {
        let colors = cx.colors().clone();
        let caps = self.caps(cx);
        h_flex()
            .flex_none()
            .h(u(32.0))
            .px(u(12.0))
            .gap(u(4.0))
            .border_b_1()
            .border_color(colors.border_variant)
            .children(View::ALL.into_iter().map(|view| {
                let active = view == self.view;
                h_flex()
                    .id(SharedString::from(format!("security-tab-{}", view.id())))
                    .px(u(10.0))
                    .h_full()
                    .items_center()
                    .cursor_pointer()
                    .text_size(u(12.5))
                    .text_color(if active { colors.text } else { colors.text_dim })
                    .when(active, |this| this.border_b_2().border_color(colors.accent))
                    .when(!view.served(&caps) && self.discovered(cx), |this| {
                        this.text_color(colors.text_faint)
                    })
                    .on_click(cx.listener(move |this, _, window, cx| this.show(view, window, cx)))
                    .child(view.label())
                    .children((self.view_criticals[view_index(view)] > 0).then(|| {
                        div()
                            .ml(u(6.0))
                            .px(u(5.0))
                            .rounded(u(8.0))
                            .bg(colors.red)
                            .text_size(u(10.5))
                            .font_family(fonts::MONO)
                            .font_weight(FontWeight::SEMIBOLD)
                            .text_color(colors.on_accent)
                            .child(self.view_criticals[view_index(view)].to_string())
                    }))
            }))
    }

    fn render_summary(&self, cx: &mut Context<Self>) -> impl IntoElement + use<> {
        let colors = cx.colors().clone();
        let totals = aggregate::totals(self.rows.borrow().counts());
        h_flex()
            .flex_none()
            .h(u(38.0))
            .px(u(12.0))
            .gap(u(8.0))
            .border_b_1()
            .border_color(colors.border_variant)
            .children(
                [
                    Severity::Critical,
                    Severity::High,
                    Severity::Medium,
                    Severity::Low,
                ]
                .map(|severity| {
                    let selected = self.filter.severity == Some(severity);
                    let color = severity_color(severity, &colors);
                    h_flex()
                        .id(SharedString::from(format!(
                            "security-sev-{}",
                            severity.label()
                        )))
                        .gap(u(5.0))
                        .cursor_pointer()
                        .on_click(cx.listener(move |this, _, _, cx| {
                            this.filter.severity = if this.filter.severity == Some(severity) {
                                None
                            } else {
                                Some(severity)
                            };
                            this.rebuild(cx);
                        }))
                        .child(Chip::new(severity.label()).dot(color).selected(selected))
                        .child(
                            div()
                                .font_family(fonts::MONO)
                                .text_size(u(12.0))
                                .text_color(color)
                                .child(totals.get(severity).to_string()),
                        )
                }),
            )
            .child(div().flex_1())
            .when(self.filter.severity.is_some(), |this| {
                this.child(
                    div()
                        .id("security-clear")
                        .text_size(u(12.0))
                        .text_color(colors.accent)
                        .cursor_pointer()
                        .on_click(cx.listener(|this, _, _, cx| {
                            this.filter.severity = None;
                            this.rebuild(cx);
                        }))
                        .child("Clear"),
                )
            })
    }

    fn render_toolbar(
        &mut self,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> impl IntoElement + use<> {
        let colors = cx.colors().clone();
        let count = self.rows.borrow().len();
        let weak = cx.entity().downgrade();
        let mut namespaces: Vec<String> = self
            .feed
            .read(cx)
            .reports(cx)
            .into_iter()
            .filter_map(|r| r.subject.namespace)
            .collect();
        namespaces.sort();
        namespaces.dedup();
        let current = self.filter.namespace.clone();
        let label = match &current {
            Some(ns) => format!("Namespace: {ns}"),
            None => "All namespaces".to_string(),
        };
        let namespace_menu = MenuButton::new("security-namespace")
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
                                    this.filter.namespace = None;
                                    this.rebuild(cx);
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
                                        this.filter.namespace = Some(value);
                                        this.rebuild(cx);
                                    })
                                    .ok();
                                }),
                        );
                    }
                    menu
                }
            });
        let menu = {
            let weak = weak.clone();
            MenuButton::new("security-menu")
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
                    .child(Icon::new(IconName::Shield).color(colors.accent))
                    .child(
                        div()
                            .text_color(colors.text)
                            .font_weight(FontWeight::MEDIUM)
                            .child("Security"),
                    )
                    .child("·")
                    .child(format!("{count} {}", self.view.label().to_lowercase())),
            )
            .child(div().flex_1())
            .child(namespace_menu)
            .child(filter)
            .child(menu)
    }

    fn render_findings(&self, cx: &mut Context<Self>) -> Option<gpui::AnyElement> {
        let colors = cx.colors().clone();
        let key = self.selected.clone()?;
        let rows = self.rows.borrow();
        let (title, counts, facts): (String, Counts, Vec<(&'static str, String)>) = match &*rows {
            Rows::None => return None,
            Rows::Images(rows) => {
                let r = rows.iter().find(|r| r.image == key)?;
                let mut facts = vec![
                    ("Scanner", r.scanner.clone()),
                    ("Workloads", r.workloads.len().to_string()),
                ];
                facts.extend(r.workloads.iter().take(8).map(|w| {
                    (
                        "",
                        match &w.container {
                            Some(c) => format!("{} · {c}", w.subject.label()),
                            None => w.subject.label(),
                        },
                    )
                }));
                if r.workloads.len() > 8 {
                    facts.push(("", format!("and {} more", r.workloads.len() - 8)));
                }
                (r.image.clone(), r.counts, facts)
            }
            Rows::Resources(rows) => {
                let r = rows.iter().find(|r| r.subject.label() == key)?;
                (
                    r.subject.label(),
                    r.counts,
                    vec![("Exposed secrets", r.secrets.to_string())],
                )
            }
            Rows::Roles(rows) => {
                let r = rows.iter().find(|r| r.subject.label() == key)?;
                (r.subject.label(), r.counts, Vec::new())
            }
        };
        drop(rows);
        let mut body = v_flex().gap(u(6.0));
        match &self.findings {
            Some((k, Findings::Loading(_))) if *k == key => {
                body = body.child(
                    div()
                        .text_color(colors.text_dim)
                        .child("Loading the report…"),
                );
            }
            Some((k, Findings::Failed(err))) if *k == key => {
                body = body.child(div().text_color(colors.red).child(err.clone()));
            }
            Some((k, Findings::Ready(details))) if *k == key => {
                body = body.child(findings_list(details, &colors));
            }
            _ => {}
        }
        let can_export =
            matches!(&self.findings, Some((k, Findings::Ready(d))) if *k == key && !d.is_empty());
        Some(
            v_flex()
                .id("security-details")
                .debug_selector(|| "security-details".into())
                .flex_none()
                .w(u(380.0))
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
                                .child(title),
                        )
                        .child(
                            h_flex()
                                .gap(u(6.0))
                                .children(
                                    [
                                        Severity::Critical,
                                        Severity::High,
                                        Severity::Medium,
                                        Severity::Low,
                                    ]
                                    .map(|s| {
                                        Chip::new(format!("{} {}", counts.get(s), s.label()))
                                            .dot(severity_color(s, &colors))
                                    }),
                                )
                                .child(div().flex_1())
                                .when(can_export, |this| {
                                    this.child(
                                        Button::new("security-export-findings")
                                            .ghost()
                                            .icon(IconName::Download)
                                            .label("CSV")
                                            .on_click(cx.listener(|this, _, _, cx| {
                                                this.export_findings(cx)
                                            })),
                                    )
                                }),
                        ),
                )
                .child(
                    v_flex()
                        .px(u(14.0))
                        .py(u(10.0))
                        .gap(u(4.0))
                        .text_size(u(12.0))
                        .border_b_1()
                        .border_color(colors.border_variant)
                        .children(facts.into_iter().map(|(k, v)| {
                            h_flex()
                                .gap(u(8.0))
                                .child(
                                    div()
                                        .flex_none()
                                        .w(u(96.0))
                                        .text_color(colors.text_dim)
                                        .child(k),
                                )
                                .child(div().flex_1().min_w_0().truncate().child(v))
                        })),
                )
                .child(v_flex().px(u(14.0)).py(u(10.0)).child(body))
                .into_any_element(),
        )
    }

    /// The empty state where no Trivy report kind is served: install or the command.
    fn render_missing(&self, cx: &mut Context<Self>) -> gpui::AnyElement {
        let colors = cx.colors().clone();
        let caps = ConnectionManager::try_global(cx)
            .map(|m| m.read(cx).caps(&self.cluster))
            .unwrap_or_default();
        let command = install::command();
        let copy = command.clone();
        v_flex()
            .id("security-missing")
            .debug_selector(|| "security-missing".into())
            .flex_1()
            .items_center()
            .justify_center()
            .gap(u(12.0))
            .p(u(24.0))
            .child(Icon::new(IconName::Shield).size(28.0).color(colors.text_dim))
            .child(
                div()
                    .text_size(u(14.0))
                    .font_weight(FontWeight::MEDIUM)
                    .child("Trivy Operator isn't installed in this cluster"),
            )
            .child(
                div()
                    .max_w(u(520.0))
                    .text_size(u(12.5))
                    .text_color(colors.text_dim)
                    .child(format!(
                        "The Security Center shows what Trivy Operator finds: vulnerabilities in images, misconfigurations of workloads, exposed secrets and risky Roles. Its report resources ({GROUP}) aren't served here."
                    )),
            )
            .child(
                h_flex()
                    .gap(u(8.0))
                    // Writes aren't offered on read-only clusters; the command stays.
                    .when(offers_install(&caps), |this| {
                        this.child(
                            Button::new("security-install")
                                .primary()
                                .icon(IconName::Download)
                                .label(format!("Install into {}…", install::NAMESPACE))
                                .on_click(cx.listener(|this, _, window, cx| this.install(window, cx))),
                        )
                    })
                    .child(
                        Button::new("security-copy-command")
                            .icon(IconName::Copy)
                            .label("Copy command")
                            .on_click(move |_, _, cx| {
                                cx.write_to_clipboard(gpui::ClipboardItem::new_string(copy.clone()));
                                NotificationCenter::push(cx, Notification::info("Copied the install command."));
                            }),
                    ),
            )
            .when(caps.read_only, |this| {
                this.child(
                    div()
                        .text_size(u(12.0))
                        .text_color(colors.text_dim)
                        .child("This cluster is read-only in Kubyl: run the command yourself."),
                )
            })
            .child(
                div()
                    .max_w(u(560.0))
                    .p(u(10.0))
                    .rounded(u(6.0))
                    .bg(colors.elevated)
                    .border_1()
                    .border_color(colors.border_variant)
                    .font_family(fonts::MONO)
                    .text_size(u(11.5))
                    .whitespace_normal()
                    .child(command),
            )
            .into_any_element()
    }

    fn empty_message(&self, cx: &App) -> Option<String> {
        if self.rows.borrow().len() > 0 {
            return None;
        }
        let feed = self.feed.read(cx);
        let caps = self.caps(cx);
        if feed.sources.is_empty() {
            return Some("Not connected to the cluster yet.".into());
        }
        if self.discovered(cx) && !self.view.served(&caps) {
            let kinds: Vec<&str> = self.view.kinds().iter().map(|k| k.plural()).collect();
            return Some(format!(
                "This cluster doesn't serve {} (Trivy Operator's {} scanner is off or too old).",
                kinds.join(" or "),
                self.view.label().to_lowercase()
            ));
        }
        if feed.loading(cx) {
            return Some("Loading reports…".into());
        }
        let problems = feed.problems(cx);
        if !problems.is_empty() {
            return Some(problems.join(" "));
        }
        let filtered = self.filter != Filter::default();
        Some(if filtered {
            "Nothing matches the filter.".into()
        } else {
            match self.view {
                View::Images => "No image has been scanned yet. Trivy Operator scans a workload's images when it starts; the first scan downloads the vulnerability database.".into(),
                View::Resources => "No config audit yet. The operator audits workloads when they change.".into(),
                View::Roles => "No RBAC assessment yet.".into(),
            }
        })
    }
}

/// Whether the empty state offers to install: never on a read-only cluster (Kubyl doesn't write
/// there; the command stays). On PROD the Helm install flow asks for the cluster's name.
pub(crate) fn offers_install(caps: &kubyl_core::ClusterCaps) -> bool {
    !caps.read_only
}

fn view_index(view: View) -> usize {
    match view {
        View::Images => 0,
        View::Resources => 1,
        View::Roles => 2,
    }
}

/// The key of each row, the identity the selection follows.
fn row_keys(rows: &Rows) -> Vec<String> {
    match rows {
        Rows::None => Vec::new(),
        Rows::Images(r) => r.iter().map(|r| r.image.clone()).collect(),
        Rows::Resources(r) => r.iter().map(|r| r.subject.label()).collect(),
        Rows::Roles(r) => r.iter().map(|r| r.subject.label()).collect(),
    }
}

fn severity_color(severity: Severity, colors: &Colors) -> gpui::Hsla {
    match severity {
        Severity::Critical => colors.red,
        Severity::High => colors.orange,
        Severity::Medium => colors.yellow,
        Severity::Low => colors.accent,
        Severity::Unknown => colors.text_dim,
    }
}

fn severity_tone(severity: Severity) -> Tone {
    match severity {
        Severity::Critical => Tone::Bad,
        Severity::High => Tone::Warning,
        Severity::Medium => Tone::Info,
        Severity::Low | Severity::Unknown => Tone::Muted,
    }
}

/// A report's findings in the panel: at most [`SHOWN`] of each list.
fn findings_list(details: &Details, colors: &Colors) -> impl IntoElement + use<> {
    let mut list = v_flex().gap(u(8.0));
    if details.is_empty() {
        return list.child(div().text_color(colors.text_dim).child("No findings."));
    }
    if !details.artifact.is_empty() {
        list = list.child(
            div().text_size(u(11.5)).text_color(colors.text_dim).child(
                format!("{} {}", details.artifact, details.os)
                    .trim()
                    .to_string(),
            ),
        );
    }
    for (ix, v) in details.vulnerabilities.iter().take(SHOWN).enumerate() {
        let link = v.link.clone();
        list = list.child(
            v_flex()
                .id(("security-vuln", ix))
                .gap(u(2.0))
                .child(
                    h_flex()
                        .gap(u(8.0))
                        .child(StatusPill::new(
                            v.severity.label(),
                            severity_tone(v.severity),
                        ))
                        .child(
                            div()
                                .id(("security-vuln-id", ix))
                                .font_family(fonts::MONO)
                                .text_size(u(12.0))
                                .when(link.is_some(), |this| {
                                    this.text_color(colors.accent)
                                        .cursor_pointer()
                                        .hover(|s| s.underline())
                                })
                                .on_click(move |_, _, cx| {
                                    if let Some(link) = &link {
                                        cx.open_url(link);
                                    }
                                })
                                .child(v.id.clone()),
                        )
                        .child(div().flex_1())
                        .children(v.score.map(|s| {
                            div()
                                .font_family(fonts::MONO)
                                .text_size(u(11.5))
                                .text_color(colors.text_dim)
                                .child(format!("{s:.1}"))
                        })),
                )
                .child(
                    div()
                        .font_family(fonts::MONO)
                        .text_size(u(11.5))
                        .text_color(colors.text_muted)
                        .child(format!(
                            "{} {} {}",
                            v.package,
                            v.installed,
                            if v.fixed.is_empty() {
                                "(no fix yet)".to_string()
                            } else {
                                format!("→ {}", v.fixed)
                            }
                        )),
                )
                .child(
                    div()
                        .text_size(u(11.5))
                        .text_color(colors.text_dim)
                        .child(v.title.clone()),
                ),
        );
    }
    for (ix, c) in details.checks.iter().take(SHOWN).enumerate() {
        list = list.child(
            v_flex()
                .id(("security-check", ix))
                .gap(u(2.0))
                .child(
                    h_flex()
                        .gap(u(8.0))
                        .child(StatusPill::new(
                            c.severity.label(),
                            severity_tone(c.severity),
                        ))
                        .child(
                            div()
                                .font_family(fonts::MONO)
                                .text_size(u(12.0))
                                .child(c.id.clone()),
                        ),
                )
                .child(div().text_size(u(12.0)).child(c.title.clone()))
                .children(c.messages.iter().take(3).map(|m| {
                    div()
                        .text_size(u(11.5))
                        .text_color(colors.text_muted)
                        .child(m.clone())
                }))
                .when(!c.remediation.is_empty(), |this| {
                    this.child(
                        div()
                            .text_size(u(11.5))
                            .text_color(colors.text_dim)
                            .child(format!("Fix: {}", c.remediation)),
                    )
                }),
        );
    }
    if !details.secrets.is_empty() {
        list =
            list.child(div().text_size(u(11.5)).text_color(colors.text_dim).child(
                "Exposed secrets: Kubyl shows the rule and the file, never the secret's text.",
            ));
    }
    for (ix, s) in details.secrets.iter().take(SHOWN).enumerate() {
        list = list.child(
            v_flex()
                .id(("security-secret", ix))
                .gap(u(2.0))
                .child(
                    h_flex()
                        .gap(u(8.0))
                        .child(StatusPill::new(
                            s.severity.label(),
                            severity_tone(s.severity),
                        ))
                        .child(
                            div()
                                .font_family(fonts::MONO)
                                .text_size(u(12.0))
                                .child(s.rule.clone()),
                        ),
                )
                .child(div().text_size(u(12.0)).child(s.title.clone()))
                .child(
                    div()
                        .font_family(fonts::MONO)
                        .text_size(u(11.5))
                        .text_color(colors.text_muted)
                        .child(s.target.clone()),
                ),
        );
    }
    if details.truncated() {
        list = list.child(
            div()
                .text_size(u(11.5))
                .text_color(colors.text_dim)
                .child(format!(
                    "The list shows the first {SHOWN}; the CSV button exports all {}.",
                    details.len()
                )),
        );
    }
    list
}

/// One report object, fetched by name (never a list).
async fn fetch_report(client: &kube::Client, report: &ReportRef) -> Result<Value, String> {
    use kube::api::{Api, DynamicObject};
    use kube::core::ApiResource;
    let resource = ApiResource {
        group: GROUP.into(),
        version: "v1alpha1".into(),
        api_version: format!("{GROUP}/v1alpha1"),
        kind: report.kind.kind().into(),
        plural: report.kind.plural().into(),
    };
    let api: Api<DynamicObject> = match &report.namespace {
        Some(ns) => Api::namespaced_with(client.clone(), ns, &resource),
        None => Api::all_with(client.clone(), &resource),
    };
    let object = api.get(&report.name).await.map_err(|err| {
        kubyl_resources_core::errors::describe(
            &err,
            "get",
            &format!("{}.{GROUP}", report.kind.plural()),
            report.namespace.as_deref(),
        )
    })?;
    serde_json::to_value(object).map_err(|e| e.to_string())
}

impl Focusable for SecurityView {
    fn focus_handle(&self, cx: &App) -> FocusHandle {
        if self.missing(cx) {
            self.focus.clone()
        } else {
            self.table.read(cx).focus_handle(cx)
        }
    }
}

impl TabView for SecurityView {
    fn tab_title(&self, _: &App) -> SharedString {
        "Security".into()
    }

    fn tab_icon(&self, _: &App) -> Option<SharedString> {
        Some(IconName::Shield.path())
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
            crate::target(&self.cluster, self.view),
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

impl Render for SecurityView {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let colors = cx.colors().clone();
        let weak_focus = self.focus.clone();
        let base = v_flex()
            .key_context(CONTEXT)
            .track_focus(&weak_focus)
            .size_full()
            .bg(colors.background)
            .text_color(colors.text)
            .font_family(fonts::UI)
            .on_action(cx.listener(|this, _: &FocusFilter, window, cx| {
                let focus = this.filter_input.read(cx).focus_handle(cx);
                focus.focus(window, cx);
            }))
            .on_action(
                cx.listener(|this, _: &ShowImages, window, cx| this.show(View::Images, window, cx)),
            )
            .on_action(cx.listener(|this, _: &ShowResources, window, cx| {
                this.show(View::Resources, window, cx)
            }))
            .on_action(
                cx.listener(|this, _: &ShowRoles, window, cx| this.show(View::Roles, window, cx)),
            )
            .on_action(cx.listener(|this, _: &ExportCsv, _, cx| this.export_csv(cx)));
        if self.missing(cx) {
            return base.child(self.render_missing(cx)).into_any_element();
        }
        let toolbar = self.render_toolbar(window, cx);
        let tabs = self.render_tabs(cx);
        let summary = self.render_summary(cx);
        let problems = self.feed.read(cx).problems(cx);
        let details = self.render_findings(cx);
        let body = match self.empty_message(cx) {
            Some(message) => div()
                .debug_selector(|| "security-empty".into())
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
                .debug_selector(|| "security-table".into())
                .child(self.table.clone())
                .into_any_element(),
        };
        let hints = vec![
            ("1/2/3".into(), "View".into()),
            ("/".into(), "Filter".into()),
        ];
        base.child(toolbar)
            .child(tabs)
            .child(summary)
            .children(
                (!problems.is_empty() && self.rows.borrow().len() > 0)
                    .then(|| banner(problems.join(" "), &colors)),
            )
            .child(
                h_flex()
                    .flex_1()
                    .min_h_0()
                    .child(v_flex().flex_1().min_w_0().h_full().child(body))
                    .children(details.map(|d| {
                        SelectionScope::new(("security-details", cx.entity_id().as_u64()), d)
                    })),
            )
            .child(KeyHints::new(hints))
            .into_any_element()
    }
}

#[cfg(test)]
mod tests {
    use std::cell::RefCell;
    use std::rc::Rc;

    use gpui::TestAppContext;
    use kubyl_core::Gvr;
    use kubyl_resources::table::parse_table;
    use kubyl_resources::{ResourceStore, ResourceStores};
    use kubyl_security_core::details;
    use kubyl_security_core::kinds::ReportKind;

    use super::*;
    use crate::feed::store_key;

    fn fixture(name: &str) -> Value {
        let path = format!(
            "{}/../kubyl_security_core/tests/fixtures/{name}",
            env!("CARGO_MANIFEST_DIR")
        );
        serde_json::from_str(&std::fs::read_to_string(path).unwrap()).unwrap()
    }

    /// The metadata objects of a recorded table (what the metadata watch holds).
    fn objects(table: &Value, labels_from: &[Value]) -> Vec<Value> {
        table["rows"]
            .as_array()
            .unwrap()
            .iter()
            .map(|row| {
                let name = row["object"]["metadata"]["name"].as_str().unwrap();
                labels_from
                    .iter()
                    .find(|o| o["metadata"]["name"] == name)
                    .cloned()
                    .map(|mut o| {
                        o.as_object_mut().unwrap().remove("report");
                        o
                    })
                    .unwrap_or_else(|| row["object"].clone())
            })
            .collect()
    }

    struct Setup {
        view: Entity<SecurityView>,
        _handles: Vec<kubyl_resources::StoreHandle>,
        _dir: tempfile::TempDir,
    }

    fn setup(cx: &mut TestAppContext, missing: bool) -> (Setup, &mut gpui::VisualTestContext) {
        let dir = tempfile::tempdir().unwrap();
        let cluster = ClusterId::new("kind-sec@/k");
        let vuln_table = fixture("table-vulnerabilityreports.json");
        let audit_table = fixture("table-configauditreports.json");
        let nginx = fixture("vulnerabilityreport-nginx.json");
        let privileged = fixture("configauditreport-privileged.json");
        let rbac = fixture("rbacassessmentreport-everything.json");
        let rbac_table = serde_json::json!({
            "columnDefinitions": [{"name": "Name"}, {"name": "Scanner"}, {"name": "Age"}, {"name": "Critical"}, {"name": "High"}, {"name": "Medium"}, {"name": "Low"}],
            "rows": [{"cells": ["role-everything", "Trivy", "9m", 2, 0, 1, 0], "object": {"metadata": {"name": "role-everything", "namespace": "kubyl-trivy"}}}]
        });
        let sets: Vec<(ReportKind, Value, Vec<Value>)> = vec![
            (
                ReportKind::Vulnerability,
                vuln_table.clone(),
                vec![nginx.clone()],
            ),
            (
                ReportKind::ConfigAudit,
                audit_table.clone(),
                vec![privileged.clone()],
            ),
            (
                ReportKind::RbacAssessment,
                rbac_table.clone(),
                vec![rbac.clone()],
            ),
        ];
        let mut kinds = Vec::new();
        let mut handles = Vec::new();
        cx.update(|cx| {
            kubyl_core::init(cx);
            kubyl_settings::init_with_dir(cx, dir.path());
            kubyl_ui::init(cx);
            for (kind, table, labels) in &sets {
                let gvr = Gvr::new(GROUP, "v1alpha1", kind.plural());
                let key = store_key(&cluster, &gvr);
                let store =
                    cx.new(|_| ResourceStore::from_objects(key.clone(), objects(table, labels)));
                handles.push(ResourceStores::insert(cx, key, store));
                kinds.push((*kind, gvr));
            }
        });
        let slot: Rc<RefCell<Option<Entity<SecurityView>>>> = Rc::default();
        let (_root, cx) = cx.add_window_view({
            let slot = slot.clone();
            let sets = sets.clone();
            move |window, cx| {
                let view = cx.new(|cx| {
                    let mut view = SecurityView::new(cluster, View::Images, window, cx);
                    view.assume_missing = missing;
                    view.feed.update(cx, |feed, cx| {
                        feed.set_kinds(kinds, cx);
                        for (kind, table, _) in &sets {
                            feed.set_table(*kind, parse_table(table), cx);
                        }
                    });
                    view
                });
                *slot.borrow_mut() = Some(view.clone());
                gpui_component::Root::new(view, window, cx)
            }
        });
        cx.run_until_parked();
        let view = slot.borrow().clone().unwrap();
        (
            Setup {
                view,
                _handles: handles,
                _dir: dir,
            },
            cx,
        )
    }

    fn names(view: &SecurityView) -> Vec<String> {
        row_keys(&view.rows.borrow())
    }

    #[gpui::test]
    fn images_list_the_worst_first_with_totals_and_findings(cx: &mut TestAppContext) {
        let (setup, cx) = setup(cx, false);
        let view = &setup.view;
        view.update(cx, |view, cx| {
            view.rebuild(cx);
            let names = names(view);
            assert_eq!(names, ["library/nginx:1.19", "library/busybox:1.37"]);
            let totals = aggregate::totals(view.rows.borrow().counts());
            assert_eq!(totals.critical, 42);
            // The sub-tabs carry each view's criticals; the sidebar badge is their sum.
            assert_eq!(view.view_criticals, [42, 0, 2]);
            assert_eq!(view.selected.as_deref(), Some("library/nginx:1.19"));
            // The panel shows what was fetched, capped; the CSV is the table.
            let details = details::parse(
                ReportKind::Vulnerability,
                &fixture("vulnerabilityreport-nginx.json"),
            );
            view.set_findings("library/nginx:1.19", details);
            let (csv, count) = view.csv_text().unwrap();
            assert_eq!(count, 2);
            assert!(
                csv.starts_with("Image,Workloads,Critical,High,Medium,Low,Secrets,Scanner,Age\r\n"),
                "{csv}"
            );
            assert!(
                csv.contains("library/nginx:1.19,1,42,143,175,28,0,Trivy,"),
                "{csv}"
            );
        });
        cx.run_until_parked();
        assert!(cx.debug_bounds("security-table").is_some());
        assert!(cx.debug_bounds("security-details").is_some());
        assert!(cx.debug_bounds("security-empty").is_none());
        assert!(cx.debug_bounds("security-missing").is_none());
    }

    #[gpui::test]
    fn severity_chips_and_the_filter_narrow_the_rows(cx: &mut TestAppContext) {
        let (setup, cx) = setup(cx, false);
        setup.view.update(cx, |view, cx| {
            view.filter.severity = Some(Severity::Critical);
            view.rebuild(cx);
            assert_eq!(names(view), ["library/nginx:1.19"]);
            view.filter.severity = None;
            view.filter.text = "busy".into();
            view.rebuild(cx);
            assert_eq!(names(view), ["library/busybox:1.37"]);
            view.filter.text = "nothing like it".into();
            view.rebuild(cx);
            assert!(names(view).is_empty());
            assert!(view.csv_text().is_none());
            assert_eq!(
                view.empty_message(cx).as_deref(),
                Some("Nothing matches the filter.")
            );
        });
        cx.run_until_parked();
        assert!(cx.debug_bounds("security-empty").is_some());
        assert!(cx.debug_bounds("security-details").is_none());
    }

    #[gpui::test]
    fn resources_and_roles_are_their_own_views(cx: &mut TestAppContext) {
        let (setup, cx) = setup(cx, false);
        let view = setup.view.clone();
        cx.update(|window, cx| view.update(cx, |view, cx| view.show(View::Resources, window, cx)));
        view.update(cx, |view, _| {
            assert_eq!(view.view(), View::Resources);
            assert_eq!(names(view).len(), 3);
            assert!(names(view).contains(&"Pod kubyl-trivy/privileged".to_string()));
            let (csv, _) = view.csv_text().unwrap();
            assert!(
                csv.starts_with("Resource,Namespace,Critical,High,Medium,Low,Secrets,Age\r\n"),
                "{csv}"
            );
        });
        cx.update(|window, cx| view.update(cx, |view, cx| view.show(View::Roles, window, cx)));
        view.update(cx, |view, _| {
            assert_eq!(names(view), ["Role kubyl-trivy/everything"]);
        });
    }

    #[test]
    fn installing_is_never_offered_on_read_only_clusters() {
        let caps = |read_only, production| kubyl_core::ClusterCaps {
            read_only,
            production,
            ..Default::default()
        };
        assert!(offers_install(&caps(false, false)));
        // PROD offers it: the Helm flow asks for the typed cluster name.
        assert!(offers_install(&caps(false, true)));
        assert!(!offers_install(&caps(true, false)));
        assert!(!offers_install(&caps(true, true)));
    }

    #[gpui::test]
    fn a_cluster_without_trivy_offers_to_install_it(cx: &mut TestAppContext) {
        let (setup, cx) = setup(cx, true);
        assert!(cx.debug_bounds("security-missing").is_some());
        assert!(cx.debug_bounds("security-table").is_none());
        let _ = &setup;
    }
}
