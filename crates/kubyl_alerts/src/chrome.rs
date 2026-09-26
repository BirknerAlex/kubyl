//! Alerts across Kubyl: the sidebar row with its badge and the red siren on cluster root rows,
//! the status bar item, the "Alerts" details section and the overview card.

use std::sync::Arc;

use gpui::{
    AnyElement, AnyView, App, AppContext as _, Context, FontWeight, IntoElement, Render,
    Subscription, Window, div, prelude::*,
};
use jiff::Timestamp;
use kubyl_core::{
    ActiveContext, ChromeRegistry, ClusterId, DetailsSection, OverviewSection, ResourceRef,
    StatusBarItem, StatusBarPosition, Tone, ViewKind,
};
use kubyl_explorer::catalog::{self, RootMarker, RowBadge, ViewRow};
use kubyl_ui::{ActiveColors, Colors, Icon, IconName, StatusDot, fonts, h_flex, u, v_flex};

use crate::merge::Heartbeat;
use crate::model::{Alert, AlertState, Severity};
use crate::service::{self, AlertsService, Counts, Pace, Phase};
use crate::settings::SidebarMode;
use crate::view::rows::ObjectFilter;
use crate::view::{self, Pending, Tab, widgets};

pub(crate) fn init(cx: &mut App) {
    catalog::register_view_row(
        cx,
        ViewRow {
            id: "alerts",
            after: "overview",
            label: "Alerts",
            icon: IconName::Siren,
            kind: ViewKind::Custom(view::VIEW_KIND.into()),
            visible: Some(Arc::new(row_visible)),
            badge: Some(Arc::new(row_badge)),
        },
    );
    catalog::register_root_marker(cx, root_marker);
    ChromeRegistry::add_status_item(cx, AlertsStatusItem);
    ChromeRegistry::add_details_section(cx, AlertsDetails);
    ChromeRegistry::add_overview_section(cx, AlertsOverview);
}

fn row_visible(cluster: &ClusterId, cx: &App) -> bool {
    let Some(service) = AlertsService::global(cx) else {
        return false;
    };
    let service = service.read(cx);
    match service.settings().sidebar {
        SidebarMode::Never => false,
        SidebarMode::Always => service.settings().enabled,
        SidebarMode::Auto => service.has_source(cluster) == Some(true),
    }
}

/// The badge: firing alerts in the color of the worst, a check when all is clear.
pub fn badge(counts: Option<Counts>) -> Option<RowBadge> {
    let counts = counts?;
    let firing = counts.firing();
    if firing == 0 {
        return Some(RowBadge::Check);
    }
    let tone = match counts.worst() {
        Some(Severity::Critical) => Tone::Bad,
        Some(Severity::Warning) => Tone::Warning,
        _ => Tone::Neutral,
    };
    Some(RowBadge::Count {
        text: firing.to_string().into(),
        tone,
    })
}

fn row_badge(cluster: &ClusterId, cx: &App) -> Option<RowBadge> {
    let service = AlertsService::global(cx)?;
    let service = service.read(cx);
    if service.phase(cluster) != Phase::Ready {
        return None;
    }
    badge(service.counts(cluster))
}

fn root_marker(cluster: &ClusterId, cx: &App) -> Option<RootMarker> {
    let counts = service::counts(cluster, cx)?;
    (counts.critical > 0).then(|| RootMarker {
        icon: IconName::Siren,
        tone: Tone::Bad,
        tooltip: format!(
            "{} critical alert{} firing",
            counts.critical,
            if counts.critical == 1 { "" } else { "s" }
        )
        .into(),
    })
}

fn active_cluster(cx: &App) -> Option<ClusterId> {
    ActiveContext::global(cx)
        .cluster
        .as_ref()
        .map(|c| c.id.clone())
}

// ----- Status bar -----

struct AlertsStatusItem;

impl StatusBarItem for AlertsStatusItem {
    fn id(&self) -> &'static str {
        "alerts"
    }

    fn position(&self) -> StatusBarPosition {
        StatusBarPosition::Left
    }

    fn order(&self) -> i32 {
        20
    }

    fn build(&self, _: &mut Window, cx: &mut App) -> AnyView {
        cx.new(AlertsStatusView::new).into()
    }
}

struct AlertsStatusView {
    _subscriptions: Vec<Subscription>,
}

impl AlertsStatusView {
    fn new(cx: &mut Context<Self>) -> Self {
        let mut subscriptions = vec![cx.observe_global::<ActiveContext>(|_, cx| cx.notify())];
        if let Some(service) = AlertsService::global(cx) {
            subscriptions.push(cx.observe(&service, |_, _, cx| cx.notify()));
        }
        Self {
            _subscriptions: subscriptions,
        }
    }
}

/// The status bar tooltip: the cluster, the count and the top three alerts.
fn status_tooltip(cluster: &ClusterId, firing: usize, cx: &App) -> AnyElement {
    let colors = cx.colors().clone();
    let now = Timestamp::now();
    let name = kubyl_kube::ConnectionManager::try_global(cx)
        .map(|m| m.read(cx).display_name(cluster).to_string())
        .unwrap_or_else(|| cluster.to_string());
    let top = service::top_alerts(cluster, 3, cx);
    v_flex()
        .gap(u(4.0))
        .text_size(u(12.0))
        .child(
            div()
                .text_color(colors.text_muted)
                .child(format!("{name} · {firing} firing")),
        )
        .children(top.iter().map(|alert| {
            h_flex()
                .gap(u(7.0))
                .child(StatusDot::new(widgets::severity_color(
                    &alert.severity,
                    &colors,
                )))
                .child(div().font_family(fonts::MONO).child(alert.name.clone()))
                .child(
                    div()
                        .max_w(u(160.0))
                        .truncate()
                        .text_color(colors.text_muted)
                        .child(
                            alert
                                .target
                                .as_ref()
                                .map(|t| t.name.clone())
                                .unwrap_or_default(),
                        ),
                )
                .child(div().flex_1())
                .child(
                    div()
                        .font_family(fonts::MONO)
                        .text_color(colors.text_dim)
                        .child(widgets::ago(alert.since(), now)),
                )
        }))
        .child(
            div()
                .text_color(colors.text_dim)
                .child(if firing > top.len() {
                    format!("and {} more · click to open Alerts", firing - top.len())
                } else {
                    "Click to open Alerts".into()
                }),
        )
        .into_any_element()
}

impl Render for AlertsStatusView {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let colors = cx.colors().clone();
        let Some(cluster) = active_cluster(cx) else {
            return div().id("alerts-status");
        };
        let Some(service) = AlertsService::global(cx) else {
            return div().id("alerts-status");
        };
        let (phase, counts) = {
            let service = service.read(cx);
            (service.phase(&cluster), service.counts(&cluster))
        };
        let Some(counts) = counts.filter(|_| phase == Phase::Ready) else {
            return div().id("alerts-status");
        };
        let firing = counts.firing();
        let color = match counts.worst() {
            Some(Severity::Critical) => colors.red,
            Some(Severity::Warning) => colors.yellow,
            Some(_) => colors.text_muted,
            None => colors.green,
        };
        let open = cluster.clone();
        let tip = cluster.clone();
        div().id("alerts-status").child(
            h_flex()
                .id("alerts-status-item")
                .gap(u(5.0))
                .cursor_pointer()
                .child(Icon::new(IconName::Siren).size(12.0).color(color))
                .child(if firing > 0 {
                    div()
                        .text_color(color)
                        .child(firing.to_string())
                        .into_any_element()
                } else {
                    Icon::new(IconName::Check)
                        .size(12.0)
                        .color(colors.green)
                        .into_any_element()
                })
                .tooltip(move |window, cx| {
                    let tip = tip.clone();
                    gpui_component::tooltip::Tooltip::element(move |_, cx| {
                        status_tooltip(&tip, firing, cx)
                    })
                    .build(window, cx)
                })
                .on_click(move |_, window, cx| view::open(&open, Some(Tab::Alerts), window, cx)),
        )
    }
}

// ----- Details section -----

struct AlertsDetails;

const DETAIL_KINDS: &[&str] = &[
    "Pod",
    "Deployment",
    "StatefulSet",
    "DaemonSet",
    "ReplicaSet",
    "Job",
    "CronJob",
    "Node",
    "Namespace",
    "PersistentVolumeClaim",
    "Service",
    "HorizontalPodAutoscaler",
];

impl DetailsSection for AlertsDetails {
    fn id(&self) -> &'static str {
        "alerts"
    }

    fn order(&self) -> i32 {
        // Above the charts.
        50
    }

    fn build(&self, target: &ResourceRef, kind: &str, cx: &mut App) -> Option<AnyView> {
        if !DETAIL_KINDS.contains(&kind) {
            return None;
        }
        AlertsService::global(cx)?;
        let object = ObjectFilter::for_resource(
            &target.gvr.resource,
            target.namespace.as_deref(),
            target.name.as_deref()?,
        )?;
        let cluster = target.cluster.clone();
        Some(cx.new(|cx| AlertsSection::new(cluster, object, cx)).into())
    }
}

struct AlertsSection {
    cluster: ClusterId,
    object: ObjectFilter,
    _subscriptions: Vec<Subscription>,
}

impl AlertsSection {
    fn new(cluster: ClusterId, object: ObjectFilter, cx: &mut Context<Self>) -> Self {
        let subscriptions = AlertsService::global(cx)
            .map(|s| vec![cx.observe(&s, |_, _, cx| cx.notify())])
            .unwrap_or_default();
        Self {
            cluster,
            object,
            _subscriptions: subscriptions,
        }
    }
}

/// One alert line: severity dot, name, since, summary; clicking opens it in the Alerts view.
fn alert_line(
    cluster: &ClusterId,
    alert: &Alert,
    index: usize,
    colors: &Colors,
    now: Timestamp,
) -> AnyElement {
    let open = cluster.clone();
    let fingerprint = alert.fingerprint.clone();
    v_flex()
        .id(("alert-line", index))
        .gap(u(2.0))
        .py(u(3.0))
        .cursor_pointer()
        .rounded(u(4.0))
        .hover(|s| s.bg(colors.hover))
        .child(
            h_flex()
                .gap(u(7.0))
                .text_size(u(12.0))
                .child(StatusDot::new(widgets::severity_color(
                    &alert.severity,
                    colors,
                )))
                .child(
                    div()
                        .flex_1()
                        .min_w_0()
                        .truncate()
                        .font_family(fonts::MONO)
                        .child(alert.name.clone()),
                )
                .when(alert.state == AlertState::Pending, |this| {
                    this.child(div().text_color(colors.yellow).child("pending"))
                })
                .child(
                    div()
                        .flex_none()
                        .font_family(fonts::MONO)
                        .text_size(u(11.0))
                        .text_color(colors.text_dim)
                        .child(widgets::ago(alert.since(), now)),
                ),
        )
        .when(!alert.summary().is_empty(), |this| {
            this.child(
                div()
                    .pl(u(14.0))
                    .truncate()
                    .text_size(u(11.5))
                    .text_color(colors.text_muted)
                    .child(alert.summary().to_string()),
            )
        })
        .on_click(move |_, window, cx| {
            view::open_with(
                Some(&open),
                Pending {
                    tab: Some(Tab::Alerts),
                    select: Some(fingerprint.clone()),
                    ..Default::default()
                },
                window,
                cx,
            )
        })
        .into_any_element()
}

impl Render for AlertsSection {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let colors = cx.colors().clone();
        let alerts = service::alerts_for(&self.cluster, &self.object, cx);
        if alerts.is_empty() {
            return div();
        }
        let now = Timestamp::now();
        v_flex()
            .px(u(14.0))
            .py(u(12.0))
            .gap(u(6.0))
            .border_b_1()
            .border_color(colors.border_variant)
            .child(
                h_flex()
                    .gap(u(6.0))
                    .text_size(u(11.0))
                    .font_weight(FontWeight::SEMIBOLD)
                    .text_color(colors.text_dim)
                    .child(Icon::new(IconName::Siren).size(11.0).color(colors.red))
                    .child(format!("ALERTS · {}", alerts.len())),
            )
            .children(
                alerts
                    .iter()
                    .enumerate()
                    .map(|(i, a)| alert_line(&self.cluster, a, i, &colors, now)),
            )
    }
}

// ----- Overview card -----

struct AlertsOverview;

impl OverviewSection for AlertsOverview {
    fn id(&self) -> &'static str {
        "alerts"
    }

    fn order(&self) -> i32 {
        10
    }

    fn build(&self, cluster: &ClusterId, namespace: Option<&str>, cx: &mut App) -> Option<AnyView> {
        AlertsService::global(cx)?;
        let cluster = cluster.clone();
        let namespace = namespace.map(str::to_string);
        Some(cx.new(|cx| AlertsCard::new(cluster, namespace, cx)).into())
    }
}

struct AlertsCard {
    cluster: ClusterId,
    namespace: Option<String>,
    _subscriptions: Vec<Subscription>,
}

impl AlertsCard {
    fn new(cluster: ClusterId, namespace: Option<String>, cx: &mut Context<Self>) -> Self {
        let subscriptions = AlertsService::global(cx)
            .map(|s| vec![cx.observe(&s, |_, _, cx| cx.notify())])
            .unwrap_or_default();
        Self {
            cluster,
            namespace,
            _subscriptions: subscriptions,
        }
    }
}

impl Render for AlertsCard {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let colors = cx.colors().clone();
        let Some(service) = AlertsService::global(cx) else {
            return div();
        };
        let (phase, alerts, heartbeat, checked) = {
            let service = service.read(cx);
            match service.cluster(&self.cluster, Pace::Background) {
                Some(state) => (
                    state.phase.clone(),
                    state.alerts.clone(),
                    state.heartbeat.clone(),
                    state.checked_at,
                ),
                None => return div(),
            }
        };
        if phase != Phase::Ready || checked.is_none() {
            return div();
        }
        let scoped: Vec<Alert> = alerts
            .iter()
            .filter(|a| {
                self.namespace
                    .as_deref()
                    .is_none_or(|ns| a.namespace() == Some(ns))
            })
            .cloned()
            .collect();
        let counts = Counts::of(&scoped);
        let now = Timestamp::now();
        let top: Vec<&Alert> = scoped
            .iter()
            .filter(|a| matches!(a.state, AlertState::Firing | AlertState::Unprocessed))
            .take(5)
            .collect();
        let open = self.cluster.clone();
        let open_ns = self.namespace.clone();
        let pill = |label: String, color, count: usize| {
            h_flex()
                .gap(u(6.0))
                .text_size(u(12.5))
                .child(StatusDot::new(color))
                .child(
                    div()
                        .font_weight(FontWeight::SEMIBOLD)
                        .child(count.to_string()),
                )
                .child(div().text_color(colors.text_muted).child(label))
        };
        let heartbeat_line = heartbeat
            .filter(|h| *h != Heartbeat::Unknown)
            .map(|h| crate::view::heartbeat(&h, now, &colors, true));
        v_flex()
            .mx(u(16.0))
            .mb(u(12.0))
            .p(u(14.0))
            .gap(u(10.0))
            .rounded(u(8.0))
            .border_1()
            .border_color(colors.border)
            .bg(colors.panel)
            .child(
                h_flex()
                    .gap(u(8.0))
                    .child(Icon::new(IconName::Siren).size(14.0).color(colors.accent))
                    .child(
                        div()
                            .font_weight(FontWeight::SEMIBOLD)
                            .child(match &self.namespace {
                                Some(ns) => format!("Alerts in {ns}"),
                                None => "Alerts".to_string(),
                            }),
                    )
                    .child(div().flex_1())
                    .child(
                        div()
                            .id("overview-alerts-open")
                            .cursor_pointer()
                            .text_size(u(12.0))
                            .text_color(colors.accent)
                            .child("Open Alerts")
                            .on_click(move |_, window, cx| {
                                let object = open_ns.as_deref().and_then(|ns| {
                                    ObjectFilter::for_resource("namespaces", None, ns)
                                });
                                view::open_with(
                                    Some(&open),
                                    Pending {
                                        tab: Some(Tab::Alerts),
                                        object,
                                        ..Default::default()
                                    },
                                    window,
                                    cx,
                                )
                            }),
                    ),
            )
            .child(if counts.firing() == 0 {
                h_flex()
                    .gap(u(8.0))
                    .text_size(u(12.5))
                    .child(
                        Icon::new(IconName::CircleCheck)
                            .size(14.0)
                            .color(colors.green),
                    )
                    .child("No alerts firing")
                    .when(counts.pending > 0, |this| {
                        this.child(
                            div()
                                .text_color(colors.yellow)
                                .child(format!("· {} pending", counts.pending)),
                        )
                    })
                    .into_any_element()
            } else {
                h_flex()
                    .gap(u(18.0))
                    .when(counts.critical > 0, |this| {
                        this.child(pill("critical".into(), colors.red, counts.critical))
                    })
                    .when(counts.warning > 0, |this| {
                        this.child(pill("warning".into(), colors.yellow, counts.warning))
                    })
                    .when(counts.info + counts.other > 0, |this| {
                        this.child(pill(
                            "info".into(),
                            colors.accent,
                            counts.info + counts.other,
                        ))
                    })
                    .when(counts.pending > 0, |this| {
                        this.child(pill("pending".into(), colors.text_dim, counts.pending))
                    })
                    .into_any_element()
            })
            .children(
                top.iter()
                    .enumerate()
                    .map(|(i, a)| alert_line(&self.cluster, a, i, &colors, now)),
            )
            .children(heartbeat_line)
    }
}
