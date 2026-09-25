//! The "Metrics" section of the details (pods, nodes, namespaces, workloads, PVCs): small charts
//! of network, disk and health series from Prometheus, with a hover crosshair and the latest
//! values in each panel's header. Registered as a `DetailsSection`.

use std::time::Duration;

use gpui::{
    AnyView, App, AppContext as _, Context, Entity, FontWeight, Global, IntoElement, Render,
    SharedString, Subscription, Task, Window, div, prelude::*,
};
use kubyl_charts::{
    ChartData, ChartKind, LineChart, Series, TimeRange, TimeRangePicker, series_color,
};
use kubyl_core::{ClusterId, DetailsSection, ResourceRef};
use kubyl_ui::{ActiveColors, Colors, fonts, h_flex, u, v_flex};

use crate::panels::{self, PanelDef};
use crate::service::{MetricsService, RangeKey, RangeState, Source};

/// How long an object must stay selected before its charts are fetched.
const SETTLE: Duration = Duration::from_millis(400);

/// The time range of every Metrics section (one choice for all objects).
#[derive(Clone, Copy, Default)]
struct DetailsRange(TimeRange);

impl Global for DetailsRange {}

pub struct MetricsDetails;

impl DetailsSection for MetricsDetails {
    fn id(&self) -> &'static str {
        "metrics"
    }

    fn order(&self) -> i32 {
        100
    }

    fn build(&self, target: &ResourceRef, kind: &str, cx: &mut App) -> Option<AnyView> {
        let (panels, filters) = panels::for_object(target, kind)?;
        MetricsService::global(cx)?;
        let cluster = target.cluster.clone();
        Some(
            cx.new(|cx| MetricsSection::new(cluster, panels, filters, cx))
                .into(),
        )
    }
}

struct Panel {
    def: &'static PanelDef,
    chart: Entity<LineChart>,
    latest: Vec<Option<f64>>,
}

pub struct MetricsSection {
    cluster: ClusterId,
    filters: Vec<(String, String)>,
    panels: Vec<Panel>,
    /// Shown long enough to fetch for (see [`SETTLE`]).
    settled: bool,
    _ticker: Task<()>,
    _subscriptions: Vec<Subscription>,
}

impl MetricsSection {
    fn new(
        cluster: ClusterId,
        defs: &'static [PanelDef],
        filters: Vec<(String, String)>,
        cx: &mut Context<Self>,
    ) -> Self {
        let panels = defs
            .iter()
            .map(|def| Panel {
                def,
                chart: cx.new(|_| {
                    let chart = LineChart::new(ChartKind::Line, move |v| def.unit.format(v))
                        .with_height(56.0)
                        .compact();
                    if def.unit.binary() {
                        chart.binary_scale()
                    } else {
                        chart
                    }
                }),
                latest: vec![None; def.series.len()],
            })
            .collect();
        let mut subscriptions =
            vec![cx.observe_global::<DetailsRange>(|this, cx| this.refresh(cx))];
        if let Some(service) = MetricsService::global(cx) {
            subscriptions.push(cx.observe(&service, |this, _, cx| this.refresh(cx)));
        }
        // Asks for the data only once the selection has settled (arrowing through a list
        // builds a section per row), then keeps asking (the cache drops what nobody reads).
        let ticker = cx.spawn(async move |this, cx| {
            cx.background_executor().timer(SETTLE).await;
            loop {
                if this
                    .update(cx, |this, cx| {
                        this.settled = true;
                        this.refresh(cx)
                    })
                    .is_err()
                {
                    break;
                }
                cx.background_executor().timer(Duration::from_secs(5)).await;
            }
        });
        Self {
            cluster,
            filters,
            panels,
            settled: false,
            _ticker: ticker,
            _subscriptions: subscriptions,
        }
    }

    fn range(cx: &App) -> TimeRange {
        cx.try_global::<DetailsRange>()
            .copied()
            .unwrap_or_default()
            .0
    }

    /// Reads (and so requests) every visible panel's series and pushes them into the charts.
    fn refresh(&mut self, cx: &mut Context<Self>) {
        if !self.settled {
            return;
        }
        let Some(service) = MetricsService::global(cx) else {
            return;
        };
        let range = Self::range(cx);
        let colors = cx.colors().clone();
        let filters: Vec<(&str, &str)> = self
            .filters
            .iter()
            .map(|(k, v)| (k.as_str(), v.as_str()))
            .collect();
        let updates: Vec<(ChartData, SharedString, Vec<Option<f64>>)> = {
            let service = service.read(cx);
            self.panels
                .iter()
                .map(|panel| {
                    if !service.has_metric(&self.cluster, panel.def.needs) {
                        return (ChartData::default(), SharedString::default(), Vec::new());
                    }
                    let mut times = Vec::new();
                    let mut series = Vec::new();
                    let mut latest = Vec::new();
                    let mut placeholder = SharedString::from(panel.def.empty);
                    for (i, def) in panel.def.series.iter().enumerate() {
                        let mut key = RangeKey::new(def.query, range);
                        for (label, value) in &filters {
                            key = key.filter(label, value);
                        }
                        let values = match service.range(&self.cluster, &key) {
                            RangeState::Ready(result) => {
                                if times.is_empty() {
                                    times = result.times();
                                }
                                result.total()
                            }
                            RangeState::Failed(err) => {
                                placeholder = err;
                                Vec::new()
                            }
                            RangeState::Loading => {
                                placeholder = "Loading…".into();
                                Vec::new()
                            }
                            RangeState::Unavailable => Vec::new(),
                        };
                        latest.push(values.iter().rev().find_map(|v| *v));
                        series.push(Series::new(
                            def.name,
                            def.name,
                            series_color(i, &colors),
                            values,
                        ));
                    }
                    (ChartData { times, series }, placeholder, latest)
                })
                .collect()
        };
        for (panel, (data, placeholder, latest)) in self.panels.iter_mut().zip(updates) {
            if latest.is_empty() {
                continue;
            }
            panel.latest = latest;
            panel.chart.update(cx, |chart, cx| {
                chart.set_placeholder(placeholder, cx);
                chart.set_data(data, cx);
            });
        }
        cx.notify();
    }

    fn panel(&self, panel: &Panel, colors: &Colors) -> impl IntoElement {
        let def = panel.def;
        let values = def
            .series
            .iter()
            .zip(&panel.latest)
            .enumerate()
            .map(|(i, (s, value))| {
                h_flex()
                    .gap(u(4.0))
                    .child(div().w(u(8.0)).h(u(2.0)).bg(series_color(i, colors)))
                    .when(!s.short.is_empty(), |this| {
                        this.child(div().text_color(colors.text_dim).child(s.short))
                    })
                    .child(
                        div().text_color(colors.text).child(
                            value
                                .map(|v| def.unit.format(v))
                                .unwrap_or_else(|| "—".into()),
                        ),
                    )
            });
        v_flex()
            .gap(u(2.0))
            .child(
                h_flex()
                    .flex_wrap()
                    .justify_between()
                    .gap_x(u(8.0))
                    .gap_y(u(2.0))
                    .child(
                        div()
                            .flex_none()
                            .text_size(u(12.0))
                            .text_color(colors.text_muted)
                            .child(def.title),
                    )
                    .child(
                        h_flex()
                            .min_w_0()
                            .gap(u(8.0))
                            .font_family(fonts::MONO)
                            .text_size(u(11.5))
                            .children(values),
                    ),
            )
            .child(panel.chart.clone())
    }
}

impl Render for MetricsSection {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let colors = cx.colors().clone();
        let range = Self::range(cx);
        let service = MetricsService::global(cx);
        let source = service
            .as_ref()
            .map(|s| s.read(cx).source(&self.cluster))
            .unwrap_or(Source::Unknown);
        let visible: Vec<&Panel> = match &service {
            Some(service) => self
                .panels
                .iter()
                .filter(|p| service.read(cx).has_metric(&self.cluster, p.def.needs))
                .collect(),
            None => Vec::new(),
        };
        let hint: Option<SharedString> = match &source {
            Source::Prometheus { .. } if visible.is_empty() => Some(
                "This Prometheus has none of the series these charts use (cAdvisor, node-exporter, kubelet volume stats)."
                    .into(),
            ),
            Source::Prometheus { .. } => None,
            Source::MetricsServer { .. } | Source::None { .. } => {
                Some("Network, disk and pressure charts need Prometheus.".into())
            }
            Source::Unknown | Source::Detecting => Some("Looking for Prometheus…".into()),
        };
        v_flex()
            .px(u(14.0))
            .py(u(12.0))
            .gap(u(10.0))
            .border_b_1()
            .border_color(colors.border_variant)
            .child(
                h_flex()
                    .gap(u(4.0))
                    .text_size(u(11.0))
                    .text_color(colors.text_dim)
                    .child(
                        div()
                            .font_weight(FontWeight::SEMIBOLD)
                            .child(format!("METRICS · LAST {}", range.label().to_uppercase())),
                    )
                    .child(div().flex_1())
                    .when(source.has_history(), |this| {
                        this.child(
                            TimeRangePicker::new("details-range", range)
                                .compact()
                                .on_change(|range, _, cx| cx.set_global(DetailsRange(range))),
                        )
                    }),
            )
            .when_some(hint, |this, hint| {
                this.child(
                    div()
                        .text_size(u(12.0))
                        .text_color(colors.text_dim)
                        .child(hint),
                )
            })
            .when(source.has_history(), |this| {
                this.children(visible.iter().map(|p| self.panel(p, &colors)))
            })
    }
}
