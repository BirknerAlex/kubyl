//! The Cost tab (board 23): window and idle controls, tiles, cost over time, the table of
//! namespaces.

use std::cell::RefCell;
use std::rc::Rc;
use std::time::Duration;

use gpui::{
    App, AppContext as _, Context, Entity, FocusHandle, Focusable, FontWeight, IntoElement,
    KeyBinding, Render, SharedString, Subscription, Task, Window, actions, div, prelude::*,
};
use gpui_component::button::{Button as MenuButton, ButtonVariants as _};
use gpui_component::menu::{DropdownMenu as _, PopupMenuItem};
use jiff::Timestamp;
use kubyl_charts::{
    ChartData, ChartKind, ColorRegistry, LineChart, Series, other_color, series_color,
};
use kubyl_core::actions::ExportCsv;
use kubyl_core::{
    ActionRegistry, ActionSpec, ActiveContext, CellValue, ClusterId, ColumnDef, ColumnWidth,
    Notification, NotificationCenter, TabView, Tone, ViewKind, ViewRequest,
};
use kubyl_cost_core::settings::{CostState, Prefs};
use kubyl_cost_core::summary::Row;
use kubyl_cost_core::table::{self, money, percent};
use kubyl_cost_core::window::{Query, Window as CostWindow};
use kubyl_kube::ConnectionManager;
use kubyl_settings::State;
use kubyl_ui::{
    ActiveColors, Button, Chip, Colors, DataTable, Icon, IconName, KeyHints, SelectionScope,
    TableDelegate, fonts, h_flex, sizes, u, v_flex,
};

use crate::service::{CostService, Data, Found};

actions!(
    cost_view,
    [
        /// Refreshes the costs now.
        Refresh,
        /// Includes or leaves out idle capacity.
        ToggleIdle,
    ]
);

/// Key context of the tab.
pub const CONTEXT: &str = "CostView";

pub(crate) fn init(cx: &mut App) {
    cx.bind_keys([
        KeyBinding::new("r", Refresh, Some(CONTEXT)),
        KeyBinding::new("i", ToggleIdle, Some(CONTEXT)),
    ]);
    ActionRegistry::register(
        cx,
        ActionSpec::new("Cost: Refresh", Refresh)
            .hint("Refresh")
            .in_context(CONTEXT),
    );
    ActionRegistry::register(
        cx,
        ActionSpec::new("Cost: Include Idle", ToggleIdle)
            .hint("Idle")
            .in_context(CONTEXT),
    );
    ActionRegistry::register(
        cx,
        ActionSpec::new("Cost: Export CSV", ExportCsv).in_context(CONTEXT),
    );
}

struct Delegate {
    rows: Rc<RefCell<Vec<Row>>>,
}

impl TableDelegate for Delegate {
    fn columns(&self) -> Vec<ColumnDef> {
        let flex = ColumnWidth::Flex {
            weight: 1.0,
            min: 160.0,
        };
        table::COLUMNS
            .iter()
            .map(|(id, title)| match *id {
                "namespace" => ColumnDef::new(*id, *title, flex).mono(),
                _ => ColumnDef::new(*id, *title, ColumnWidth::Fixed(96.0))
                    .align_end()
                    .mono(),
            })
            .collect()
    }

    fn row_count(&self, _: &App) -> usize {
        self.rows.borrow().len()
    }

    fn cell(&self, row: usize, column: usize, _: &App) -> CellValue {
        let rows = self.rows.borrow();
        let Some(r) = rows.get(row) else {
            return CellValue::Empty;
        };
        let id = table::COLUMNS[column].0;
        let text = table::cell(r, id);
        match id {
            "namespace" if r.is_idle() => CellValue::Tinted {
                label: text.into(),
                tone: Tone::Muted,
            },
            "efficiency" if text.is_empty() => CellValue::Empty,
            "efficiency" => CellValue::Tinted {
                label: text.into(),
                tone: efficiency_tone(r.efficiency.unwrap_or(0.0)),
            },
            _ => CellValue::Text(text.into()),
        }
    }
}

/// Low usage of what was requested is the waste: red below a quarter, yellow below half.
fn efficiency_tone(efficiency: f64) -> Tone {
    if efficiency < 0.25 {
        Tone::Bad
    } else if efficiency < 0.5 {
        Tone::Warning
    } else {
        Tone::Good
    }
}

pub struct CostView {
    cluster: ClusterId,
    prefs: Prefs,
    service: Option<Entity<CostService>>,
    chart: Entity<LineChart>,
    table: Entity<DataTable>,
    rows: Rc<RefCell<Vec<Row>>>,
    focus: FocusHandle,
    _subscriptions: Vec<Subscription>,
    _keepalive: Task<()>,
}

impl CostView {
    pub fn new(cluster: ClusterId, _: &mut Window, cx: &mut Context<Self>) -> Self {
        let prefs = Self::load_prefs(&cluster, cx);
        let rows = Rc::new(RefCell::new(Vec::new()));
        let table = cx.new(|cx| DataTable::new(Delegate { rows: rows.clone() }, cx));
        let chart = cx.new(|_| LineChart::new(ChartKind::StackedArea, money).with_height(170.0));
        let service = CostService::global(cx);
        let mut subscriptions = Vec::new();
        if let Some(service) = &service {
            subscriptions.push(cx.observe(service, |this, _, cx| this.refresh_data(cx)));
        }
        // A want lapses after 45 s: say it again while the tab lives.
        let keepalive = cx.spawn(async move |this, cx| {
            loop {
                cx.background_executor()
                    .timer(Duration::from_secs(20))
                    .await;
                if this.update(cx, |this, cx| this.want(cx)).is_err() {
                    break;
                }
            }
        });
        let mut this = Self {
            cluster,
            prefs,
            service,
            chart,
            table,
            rows,
            focus: cx.focus_handle(),
            _subscriptions: subscriptions,
            _keepalive: keepalive,
        };
        this.want(cx);
        this.refresh_data(cx);
        this
    }

    fn key(cluster: &ClusterId, cx: &App) -> String {
        ConnectionManager::try_global(cx)
            .and_then(|m| m.read(cx).context(cluster).map(|c| c.stable_key()))
            .unwrap_or_else(|| cluster.to_string())
    }

    fn load_prefs(cluster: &ClusterId, cx: &App) -> Prefs {
        State::get::<CostState>(cx)
            .clusters
            .get(&Self::key(cluster, cx))
            .copied()
            .unwrap_or_default()
    }

    fn save_prefs(&self, cx: &mut App) {
        let key = Self::key(&self.cluster, cx);
        let prefs = self.prefs;
        State::update::<CostState>(cx, |state| {
            state.clusters.insert(key, prefs);
        });
    }

    fn query(&self) -> Query {
        Query {
            window: self.prefs.window,
            include_idle: self.prefs.include_idle,
            accumulate: true,
        }
    }

    fn want(&mut self, cx: &mut Context<Self>) {
        let (cluster, query) = (self.cluster.clone(), self.query());
        if let Some(service) = &self.service {
            service.update(cx, |service, cx| service.want(&cluster, query, cx));
        }
    }

    pub fn set_window(&mut self, window: CostWindow, cx: &mut Context<Self>) {
        if self.prefs.window != window {
            self.prefs.window = window;
            self.save_prefs(cx);
            self.want(cx);
            cx.notify();
        }
    }

    pub fn toggle_idle(&mut self, cx: &mut Context<Self>) {
        self.prefs.include_idle = !self.prefs.include_idle;
        self.save_prefs(cx);
        self.want(cx);
        cx.notify();
    }

    fn refresh_now(&mut self, cx: &mut Context<Self>) {
        let cluster = self.cluster.clone();
        if let Some(service) = &self.service {
            service.update(cx, |service, cx| service.refresh(&cluster, cx));
        }
    }

    pub fn prefs(&self) -> Prefs {
        self.prefs
    }

    /// The data shown: what the service has, when it's for the chosen window and idle choice
    /// (else the table waits for the new read).
    fn data(&self, cx: &App) -> Option<Data> {
        let service = self.service.as_ref()?;
        let data = service.read(cx).cluster(&self.cluster)?.data.clone()?;
        let wanted = self.query();
        (data.query.window == wanted.window && data.query.include_idle == wanted.include_idle)
            .then_some(data)
    }

    /// Pushes the service's data into the table and the chart.
    fn refresh_data(&mut self, cx: &mut Context<Self>) {
        let data = self.data(cx);
        *self.rows.borrow_mut() = data.as_ref().map(|d| d.rows.clone()).unwrap_or_default();
        self.table.update(cx, |table, cx| {
            let first = (!self.rows.borrow().is_empty()).then_some(0);
            table.select(table.selected().or(first), cx);
            cx.notify();
        });
        let colors = cx.colors().clone();
        let chart_data = match &data {
            Some(data) if !data.series.is_empty() => {
                let names: Vec<SharedString> = data
                    .series
                    .stacks
                    .iter()
                    .filter(|(n, _)| n != "other" && n != kubyl_cost_core::model::IDLE)
                    .map(|(n, _)| SharedString::from(n.clone()))
                    .collect();
                let slots =
                    ColorRegistry::assign(cx, &format!("{}/namespace", self.cluster), &names);
                let mut slot = slots.into_iter();
                let series = data
                    .series
                    .stacks
                    .iter()
                    .map(|(name, values)| {
                        let values: Vec<Option<f64>> = values.iter().map(|v| Some(*v)).collect();
                        match name.as_str() {
                            "other" => {
                                Series::new("__other", "other", other_color(&colors), values)
                                    .dashed()
                            }
                            kubyl_cost_core::model::IDLE => {
                                Series::new("__idle", "idle", colors.text_faint, values)
                            }
                            name => Series::new(
                                name.to_string(),
                                name.to_string(),
                                series_color(slot.next().unwrap_or(0), &colors),
                                values,
                            ),
                        }
                    })
                    .collect();
                ChartData {
                    times: data.series.times.clone(),
                    series,
                }
            }
            _ => ChartData::default(),
        };
        let placeholder = self.placeholder(cx);
        self.chart.update(cx, |chart, cx| {
            chart.set_placeholder(placeholder, cx);
            chart.set_data(chart_data, cx);
        });
        cx.notify();
    }

    fn placeholder(&self, cx: &App) -> SharedString {
        let state = self
            .service
            .as_ref()
            .and_then(|s| s.read(cx).cluster(&self.cluster));
        match state {
            Some(state) if state.error.is_some() && state.data.is_none() => "No data.".into(),
            Some(state) if state.data.is_some() => "No cost in this window yet.".into(),
            _ => "Loading…".into(),
        }
    }

    /// The CSV of the table as shown, and how many rows it has.
    pub fn csv_text(&self) -> Option<(String, usize)> {
        let rows = self.rows.borrow();
        if rows.is_empty() {
            return None;
        }
        let records = table::records(rows.iter());
        Some((
            kubyl_core::csv::write(&table::header(), &records),
            records.len(),
        ))
    }

    fn export_csv(&mut self, cx: &mut Context<Self>) {
        let Some((text, count)) = self.csv_text() else {
            NotificationCenter::push(cx, Notification::info("There are no costs to export."));
            return;
        };
        let file = kubyl_core::export::file_name(
            &format!("cost-{}", self.prefs.window.param()),
            "csv",
            Timestamp::now(),
        );
        kubyl_core::export::save_text(cx, &file, format!("{count} rows"), text);
    }

    // ----- Rendering -----

    fn render_toolbar(&self, cx: &mut Context<Self>) -> impl IntoElement + use<> {
        let colors = cx.colors().clone();
        let weak = cx.entity().downgrade();
        let updated = self
            .data(cx)
            .map(|d| age(d.fetched))
            .map(|a| format!("updated {a} ago"));
        let loading = self
            .service
            .as_ref()
            .and_then(|s| s.read(cx).cluster(&self.cluster).map(|c| c.loading()))
            .unwrap_or(false);
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
                    .child(Icon::new(IconName::Cloud).color(colors.accent))
                    .child(
                        div()
                            .text_color(colors.text)
                            .font_weight(FontWeight::MEDIUM)
                            .child("Cost"),
                    )
                    .child("·")
                    .child(format!("last {}", self.prefs.window.label())),
            )
            .child(div().flex_1())
            .children(updated.map(|t| {
                div()
                    .text_size(u(11.5))
                    .text_color(colors.text_dim)
                    .child(t)
            }))
            .children(CostWindow::ALL.map(|window| {
                let selected = self.prefs.window == window;
                div()
                    .id(SharedString::from(format!(
                        "cost-window-{}",
                        window.param()
                    )))
                    .cursor_pointer()
                    .on_click(cx.listener(move |this, _, _, cx| this.set_window(window, cx)))
                    .child(Chip::new(window.param()).selected(selected))
            }))
            .child(
                div()
                    .id("cost-idle")
                    .cursor_pointer()
                    .on_click(cx.listener(|this, _, _, cx| this.toggle_idle(cx)))
                    .child(Chip::new("include idle").selected(self.prefs.include_idle)),
            )
            .child(
                Button::new("cost-refresh")
                    .ghost()
                    .icon(IconName::RefreshCw)
                    .disabled(loading)
                    .on_click(cx.listener(|this, _, _, cx| this.refresh_now(cx))),
            )
            .child(
                MenuButton::new("cost-menu")
                    .ghost()
                    .compact()
                    .child(Icon::new(IconName::SlidersVertical).size(13.0))
                    .dropdown_menu(move |menu, _, _| {
                        let weak = weak.clone();
                        menu.item(PopupMenuItem::new("Export CSV…").on_click(move |_, _, cx| {
                            weak.update(cx, |this, cx| this.export_csv(cx)).ok();
                        }))
                    }),
            )
    }

    fn render_tiles(&self, data: &Data, cx: &mut Context<Self>) -> impl IntoElement + use<> {
        let colors = cx.colors().clone();
        let t = data.totals;
        let tile = |id: &'static str,
                    label: &'static str,
                    value: String,
                    hint: String,
                    color: gpui::Hsla| {
            v_flex()
                .id(id)
                .flex_1()
                .min_w(u(150.0))
                .p(u(12.0))
                .gap(u(2.0))
                .rounded(u(8.0))
                .border_1()
                .border_color(colors.border)
                .bg(colors.panel)
                .child(
                    div()
                        .text_size(u(11.0))
                        .text_color(colors.text_dim)
                        .child(label.to_uppercase()),
                )
                .child(
                    div()
                        .font_family(fonts::MONO)
                        .text_size(u(20.0))
                        .text_color(color)
                        .child(value),
                )
                .child(
                    div()
                        .text_size(u(11.5))
                        .text_color(colors.text_dim)
                        .child(hint),
                )
        };
        let breakdown = format!(
            "CPU {} · memory {} · storage {} · network {}",
            money(t.cpu),
            money(t.ram),
            money(t.storage),
            money(t.network)
        );
        let idle_hint = if data.query.include_idle {
            format!(
                "{} of the total",
                percent(Some(if t.total > 0.0 { t.idle / t.total } else { 0.0 }))
            )
        } else {
            "left out (include idle)".to_string()
        };
        h_flex()
            .flex_none()
            .gap(u(10.0))
            .px(u(12.0))
            .py(u(10.0))
            .child(tile(
                "cost-total",
                "Total",
                money(t.total),
                breakdown,
                colors.text,
            ))
            .child(tile(
                "cost-idle-tile",
                "Idle",
                money(t.idle),
                idle_hint,
                colors.text_muted,
            ))
            .child(tile(
                "cost-cpu-eff",
                "CPU efficiency",
                percent(Some(t.cpu_efficiency)),
                "usage of the requested cores".into(),
                efficiency_color(t.cpu_efficiency, &colors),
            ))
            .child(tile(
                "cost-ram-eff",
                "Memory efficiency",
                percent(Some(t.ram_efficiency)),
                "usage of the requested memory".into(),
                efficiency_color(t.ram_efficiency, &colors),
            ))
    }

    /// Why there is nothing to show, when there is nothing.
    fn empty_state(&self, cx: &App) -> Option<gpui::AnyElement> {
        let colors = cx.colors().clone();
        let state = self
            .service
            .as_ref()
            .and_then(|s| s.read(cx).cluster(&self.cluster));
        let (title, body, command): (String, String, Option<String>) = match state.map(|s| &s.found) {
            None | Some(Found::Unknown) | Some(Found::Detecting) => {
                ("Looking for OpenCost…".into(), String::new(), None)
            }
            Some(Found::Missing) => (
                "OpenCost isn't installed in this cluster".into(),
                "The Cost view shows what OpenCost computes: costs per namespace, idle capacity and how well requested CPU and memory are used. OpenCost needs Prometheus (it reads the cluster's metrics from it), so install or point it at one first. If it runs under another name, set `cost.clusters.<cluster>.service` in settings.json to namespace/service:port.".into(),
                Some(INSTALL.into()),
            ),
            Some(Found::Failed(why)) => ("Couldn't look for OpenCost".into(), why.clone(), None),
            Some(Found::At(_)) => {
                let state = state?;
                // The filtered data: the last window's numbers don't count for this one.
                match (self.data(cx), &state.error) {
                    (Some(_), _) => return None,
                    (None, Some(error)) => (
                        "OpenCost doesn't answer".into(),
                        error.to_string(),
                        None,
                    ),
                    (None, None) => ("Reading the costs…".into(), String::new(), None),
                }
            }
        };
        Some(
            v_flex()
                .id("cost-empty")
                .debug_selector(|| "cost-empty".into())
                .flex_1()
                .items_center()
                .justify_center()
                .gap(u(10.0))
                .p(u(24.0))
                .child(Icon::new(IconName::Cloud).size(26.0).color(colors.text_dim))
                .child(
                    div()
                        .text_size(u(14.0))
                        .font_weight(FontWeight::MEDIUM)
                        .child(title),
                )
                .when(!body.is_empty(), |this| {
                    this.child(
                        div()
                            .max_w(u(560.0))
                            .text_size(u(12.5))
                            .text_color(colors.text_dim)
                            .child(body),
                    )
                })
                .children(command.map(|command| {
                    div()
                        .max_w(u(560.0))
                        .p(u(10.0))
                        .rounded(u(6.0))
                        .bg(colors.elevated)
                        .border_1()
                        .border_color(colors.border_variant)
                        .font_family(fonts::MONO)
                        .text_size(u(11.5))
                        .child(command)
                }))
                .into_any_element(),
        )
    }
}

/// How to install OpenCost on a cluster that has Prometheus (the chart's own instructions).
pub const INSTALL: &str = "helm install opencost opencost \\\n  --repo https://opencost.github.io/opencost-helm-chart \\\n  --namespace opencost --create-namespace \\\n  --set opencost.prometheus.internal.serviceName=<prometheus service> \\\n  --set opencost.prometheus.internal.namespaceName=<its namespace> \\\n  --set opencost.prometheus.internal.port=9090";

fn efficiency_color(efficiency: f64, colors: &Colors) -> gpui::Hsla {
    match efficiency_tone(efficiency) {
        Tone::Bad => colors.red,
        Tone::Warning => colors.yellow,
        _ => colors.green,
    }
}

/// `12s`, `3m`.
fn age(at: Timestamp) -> String {
    let seconds = (Timestamp::now().as_second() - at.as_second()).max(0);
    if seconds < 60 {
        format!("{seconds}s")
    } else {
        format!("{}m", seconds / 60)
    }
}

impl Focusable for CostView {
    fn focus_handle(&self, cx: &App) -> FocusHandle {
        if self.rows.borrow().is_empty() {
            self.focus.clone()
        } else {
            self.table.read(cx).focus_handle(cx)
        }
    }
}

impl TabView for CostView {
    fn tab_title(&self, _: &App) -> SharedString {
        "Cost".into()
    }

    fn tab_icon(&self, _: &App) -> Option<SharedString> {
        Some(IconName::Cloud.path())
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

impl Render for CostView {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let colors = cx.colors().clone();
        let toolbar = self.render_toolbar(cx);
        let data = self.data(cx);
        let error = self.service.as_ref().and_then(|s| {
            s.read(cx)
                .cluster(&self.cluster)
                .and_then(|c| c.error.clone())
        });
        let body = match (data.as_ref(), self.empty_state(cx)) {
            (Some(data), _) => {
                let tiles = self.render_tiles(data, cx);
                v_flex()
                    .flex_1()
                    .min_h_0()
                    .child(tiles)
                    .child(
                        v_flex()
                            .flex_none()
                            .mx(u(12.0))
                            .mb(u(8.0))
                            .p(u(12.0))
                            .gap(u(6.0))
                            .rounded(u(8.0))
                            .border_1()
                            .border_color(colors.border)
                            .bg(colors.panel)
                            .debug_selector(|| "cost-chart".into())
                            .child(div().text_size(u(11.0)).text_color(colors.text_dim).child(
                                format!(
                                    "COST OVER TIME · PER {}",
                                    if data.query.window == CostWindow::Day {
                                        "HOUR"
                                    } else {
                                        "DAY"
                                    }
                                ),
                            ))
                            .child(self.chart.clone()),
                    )
                    .child(
                        div()
                            .flex_1()
                            .min_h_0()
                            .flex()
                            .debug_selector(|| "cost-table".into())
                            .child(self.table.clone()),
                    )
                    .into_any_element()
            }
            (None, Some(empty)) => empty,
            (None, None) => div().flex_1().into_any_element(),
        };
        let hints = vec![("r".into(), "Refresh".into()), ("i".into(), "Idle".into())];
        v_flex()
            .key_context(CONTEXT)
            .track_focus(&self.focus)
            .size_full()
            .bg(colors.background)
            .text_color(colors.text)
            .font_family(fonts::UI)
            .on_action(cx.listener(|this, _: &Refresh, _, cx| this.refresh_now(cx)))
            .on_action(cx.listener(|this, _: &ToggleIdle, _, cx| this.toggle_idle(cx)))
            .on_action(cx.listener(|this, _: &ExportCsv, _, cx| this.export_csv(cx)))
            .child(toolbar)
            .children(error.filter(|_| data.is_some()).map(|error| {
                h_flex()
                    .flex_none()
                    .px(u(12.0))
                    .py(u(6.0))
                    .gap(u(8.0))
                    .bg(colors.yellow.opacity(0.1))
                    .border_b_1()
                    .border_color(colors.yellow.opacity(0.35))
                    .text_size(u(12.0))
                    .debug_selector(|| "cost-error".into())
                    .child(
                        Icon::new(IconName::TriangleAlert)
                            .size(12.0)
                            .color(colors.yellow),
                    )
                    .child(format!("{error} Showing the last read."))
            }))
            .child(SelectionScope::new(
                ("cost", cx.entity_id().as_u64()),
                v_flex().flex_1().min_h_0().child(body),
            ))
            .child(KeyHints::new(hints))
    }
}

#[cfg(test)]
mod tests {
    use std::cell::RefCell;
    use std::rc::Rc;

    use gpui::TestAppContext;
    use kubyl_cost_core::fetch::CostError;
    use kubyl_cost_core::model::parse;
    use kubyl_cost_core::settings::Endpoint;
    use serde_json::json;

    use super::*;
    use crate::service::build;

    fn allocation(name: &str, total: f64, start: &str) -> serde_json::Value {
        json!({"name": name, "cpuCost": total * 0.6, "ramCost": total * 0.3, "pvCost": total * 0.1, "networkCost": 0.0,
            "totalCost": total, "cpuEfficiency": 0.2, "ramEfficiency": 0.6, "totalEfficiency": 0.4,
            "window": {"start": start, "end": start}})
    }

    fn data(query: Query) -> Data {
        let accumulated = parse(&json!({"code": 200, "data": [{
            "shop": allocation("shop", 6.0, "2026-10-09T00:00:00Z"),
            "bank": allocation("bank", 3.0, "2026-10-09T00:00:00Z"),
            "__idle__": {"name": "__idle__", "cpuCost": 2.0, "totalCost": 2.0},
        }]}))
        .unwrap();
        let steps = parse(&json!({"code": 200, "data": [
            {"shop": allocation("shop", 3.0, "2026-10-09T00:00:00Z"), "bank": allocation("bank", 1.0, "2026-10-09T00:00:00Z")},
            {"shop": allocation("shop", 3.0, "2026-10-09T01:00:00Z"), "bank": allocation("bank", 2.0, "2026-10-09T01:00:00Z")},
        ]}))
        .unwrap();
        build(query, &accumulated, &steps)
    }

    struct Setup {
        view: Entity<CostView>,
        service: Entity<CostService>,
        cluster: ClusterId,
        _dir: tempfile::TempDir,
    }

    fn setup(cx: &mut TestAppContext) -> (Setup, &mut gpui::VisualTestContext) {
        let dir = tempfile::tempdir().unwrap();
        let cluster = ClusterId::new("kind-cost@/k");
        let service = cx.update(|cx| {
            kubyl_core::init(cx);
            kubyl_settings::init_with_dir(cx, dir.path());
            kubyl_ui::init(cx);
            kubyl_settings::Settings::register::<kubyl_cost_core::settings::CostSettings>(cx);
            CostService::install(false, cx)
        });
        let slot: Rc<RefCell<Option<Entity<CostView>>>> = Rc::default();
        let (_root, cx) = cx.add_window_view({
            let (slot, cluster) = (slot.clone(), cluster.clone());
            move |window, cx| {
                let view = cx.new(|cx| CostView::new(cluster, window, cx));
                *slot.borrow_mut() = Some(view.clone());
                gpui_component::Root::new(view, window, cx)
            }
        });
        cx.run_until_parked();
        let view = slot.borrow().clone().unwrap();
        (
            Setup {
                view,
                service,
                cluster,
                _dir: dir,
            },
            cx,
        )
    }

    fn found() -> Found {
        Found::At("opencost/opencost:9003".parse::<Endpoint>().unwrap())
    }

    #[gpui::test]
    fn costs_show_tiles_chart_and_table(cx: &mut TestAppContext) {
        let (s, cx) = setup(cx);
        let query = s.view.read_with(cx, |v, _| v.query());
        assert!(
            cx.debug_bounds("cost-empty").is_some(),
            "waiting for OpenCost"
        );
        s.service.update(cx, |service, cx| {
            service.set(&s.cluster, found(), Some(data(query)), None, cx)
        });
        cx.run_until_parked();
        assert!(cx.debug_bounds("cost-chart").is_some());
        assert!(cx.debug_bounds("cost-table").is_some());
        assert!(cx.debug_bounds("cost-empty").is_none());
        assert!(cx.debug_bounds("cost-error").is_none());
        s.view.read_with(cx, |view, cx| {
            let rows: Vec<String> = view
                .rows
                .borrow()
                .iter()
                .map(|r| r.label().to_string())
                .collect();
            assert_eq!(rows, ["shop", "bank", "idle"]);
            let (csv, count) = view.csv_text().unwrap();
            assert_eq!(count, 3);
            assert!(
                csv.starts_with("Namespace,CPU,Memory,Storage,Network,Total,Efficiency\r\n"),
                "{csv}"
            );
            assert!(
                csv.contains("shop,3.6000,1.8000,0.6000,0.0000,6.0000,0.4000"),
                "{csv}"
            );
            // The chart stacks both namespaces and idle over the two steps.
            let chart = view.chart.read(cx).data();
            let names: Vec<String> = chart.series.iter().map(|s| s.name.to_string()).collect();
            assert_eq!(
                names,
                ["shop", "bank"].map(String::from).to_vec(),
                "{names:?}"
            );
            assert_eq!(chart.times.len(), 2);
        });
    }

    #[gpui::test]
    fn window_and_idle_are_remembered_per_cluster(cx: &mut TestAppContext) {
        let (s, cx) = setup(cx);
        s.view.update(cx, |view, cx| {
            assert_eq!(view.prefs(), Prefs::default());
            view.set_window(CostWindow::Month, cx);
            view.toggle_idle(cx);
            assert_eq!(view.prefs().window, CostWindow::Month);
            assert!(!view.prefs().include_idle);
            assert_eq!(view.query().window, CostWindow::Month);
            assert!(!view.query().include_idle);
        });
        // Saved under the cluster; another view of it starts with the choice.
        cx.update(|_, cx| {
            let state = State::get::<CostState>(cx);
            let saved = state.clusters.get(&s.cluster.to_string()).copied().unwrap();
            assert_eq!(saved.window, CostWindow::Month);
            assert!(!saved.include_idle);
            assert_eq!(CostView::load_prefs(&s.cluster, cx), saved);
            assert_eq!(
                CostView::load_prefs(&ClusterId::new("other@/k"), cx),
                Prefs::default()
            );
        });
        // Data for another window isn't shown under this one's name.
        let stale = Query {
            window: CostWindow::Day,
            include_idle: true,
            accumulate: true,
        };
        s.service.update(cx, |service, cx| {
            service.set(&s.cluster, found(), Some(data(stale)), None, cx)
        });
        cx.run_until_parked();
        s.view.read_with(cx, |view, cx| {
            assert!(view.data(cx).is_none());
            assert!(view.empty_state(cx).is_some(), "waiting, not blank");
        });
        // A failed read of the new window says so instead of leaving the body empty.
        s.service.update(cx, |service, cx| {
            service.set(
                &s.cluster,
                found(),
                Some(data(stale)),
                Some(CostError::Unreachable("timed out".into())),
                cx,
            )
        });
        cx.run_until_parked();
        assert!(cx.debug_bounds("cost-empty").is_some());
    }

    #[gpui::test]
    fn empty_states_say_what_to_do(cx: &mut TestAppContext) {
        let (s, cx) = setup(cx);
        let query = s.view.read_with(cx, |v, _| v.query());
        let set = |cx: &mut gpui::VisualTestContext, found, data, error| {
            s.service.update(cx, |service, cx| {
                service.set(&s.cluster, found, data, error, cx)
            });
            cx.run_until_parked();
        };
        set(cx, Found::Missing, None, None);
        assert!(cx.debug_bounds("cost-empty").is_some());
        s.view.read_with(cx, |view, cx| {
            assert!(view.empty_state(cx).is_some());
        });
        set(
            cx,
            Found::Failed("Forbidden: you can't list services cluster-wide.".into()),
            None,
            None,
        );
        assert!(cx.debug_bounds("cost-empty").is_some());
        set(
            cx,
            found(),
            None,
            Some(CostError::Unreachable(
                "opencost/opencost has no ready pods (503).".into(),
            )),
        );
        assert!(cx.debug_bounds("cost-empty").is_some());
        // An error with numbers on screen keeps them, with the error above.
        set(
            cx,
            found(),
            Some(data(query)),
            Some(CostError::OpenCost("OpenCost can't read Prometheus".into())),
        );
        assert!(cx.debug_bounds("cost-error").is_some());
        assert!(cx.debug_bounds("cost-table").is_some());
        assert!(cx.debug_bounds("cost-empty").is_none());
    }

    #[test]
    fn the_install_hint_names_prometheus() {
        assert!(INSTALL.contains("opencost.prometheus.internal.serviceName"));
        assert!(INSTALL.contains("https://opencost.github.io/opencost-helm-chart"));
        assert_eq!(efficiency_tone(0.1), Tone::Bad);
        assert_eq!(efficiency_tone(0.4), Tone::Warning);
        assert_eq!(efficiency_tone(0.9), Tone::Good);
    }
}
