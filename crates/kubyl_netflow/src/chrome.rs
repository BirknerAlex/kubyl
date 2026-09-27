//! Network flows across Kubyl: the sidebar row under every cluster, the namespace entry of
//! favorites, and the "Network flows" details section of pods, workloads, Services and
//! namespaces (it reads the streams an open view runs; it never starts one).

use std::sync::Arc;
use std::time::Duration;

use gpui::{
    AnyView, App, AppContext as _, Context, FontWeight, IntoElement, Render, Task, Window, div,
    prelude::*,
};
use kubyl_core::{ChromeRegistry, ClusterId, DetailsSection, ResourceRef, ViewKind};
use kubyl_explorer::catalog::{self, NamespaceView, ViewRow};
use kubyl_ui::{ActiveColors, IconName, h_flex, u, v_flex};

use crate::filter::FlowFilter;
use crate::service::FlowService;
use crate::settings::NetflowSettings;
use crate::view::{self, Pending, Tab, widgets};

pub(crate) fn init(cx: &mut App) {
    catalog::register_view_row(
        cx,
        ViewRow {
            id: "network_flows",
            after: "events",
            label: "Network Flows",
            icon: IconName::Waypoints,
            kind: ViewKind::Custom(view::VIEW_KIND.into()),
            visible: Some(Arc::new(|_: &ClusterId, cx: &App| {
                !cx.has_global::<kubyl_settings::Settings>()
                    || kubyl_settings::Settings::get::<NetflowSettings>(cx).sidebar
            })),
            badge: None,
        },
    );
    catalog::register_namespace_view(
        cx,
        NamespaceView {
            label: "Open Namespace Network Flows",
            kind: ViewKind::Custom(view::VIEW_KIND.into()),
        },
    );
    ChromeRegistry::add_details_section(cx, FlowsDetails);
}

const DETAIL_KINDS: &[&str] = &[
    "Pod",
    "Deployment",
    "StatefulSet",
    "DaemonSet",
    "ReplicaSet",
    "Job",
    "Service",
    "Namespace",
];

struct FlowsDetails;

impl DetailsSection for FlowsDetails {
    fn id(&self) -> &'static str {
        "network_flows"
    }

    fn order(&self) -> i32 {
        // After alerts, before the charts.
        60
    }

    fn build(&self, target: &ResourceRef, kind: &str, cx: &mut App) -> Option<AnyView> {
        if !DETAIL_KINDS.contains(&kind) {
            return None;
        }
        FlowService::global(cx)?;
        let query = crate::actions::query_for(target)?;
        let filter = FlowFilter::parse(&query).ok()?;
        let cluster = target.cluster.clone();
        Some(
            cx.new(|cx| FlowsSection::new(cluster, query, filter, cx))
                .into(),
        )
    }
}

/// What the open streams hold about an object.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
struct Summary {
    flows: usize,
    blocked: usize,
}

struct FlowsSection {
    cluster: ClusterId,
    query: String,
    filter: FlowFilter,
    summary: Option<Summary>,
    _ticker: Task<()>,
}

impl FlowsSection {
    fn new(cluster: ClusterId, query: String, filter: FlowFilter, cx: &mut Context<Self>) -> Self {
        let ticker = cx.spawn(async move |this, cx| {
            loop {
                if this.update(cx, |this, cx| this.recount(cx)).is_err() {
                    break;
                }
                cx.background_executor().timer(Duration::from_secs(2)).await;
            }
        });
        Self {
            cluster,
            query,
            filter,
            summary: None,
            _ticker: ticker,
        }
    }

    fn recount(&mut self, cx: &mut Context<Self>) {
        let Some(service) = FlowService::global(cx) else {
            return;
        };
        let service = service.read(cx);
        let mut summary: Option<Summary> = None;
        // The widest stream holds the most; streams overlap, so take the best one.
        for stream in service.streams(&self.cluster) {
            let mut here = Summary::default();
            for (_, flow) in stream.buffer.since(0) {
                if self.filter.matches(flow) {
                    here.flows += 1;
                    here.blocked += usize::from(flow.verdict.blocked());
                }
            }
            if summary.is_none_or(|s| here.flows > s.flows) {
                summary = Some(here);
            }
        }
        if summary != self.summary {
            self.summary = summary;
            cx.notify();
        }
    }
}

impl Render for FlowsSection {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let colors = cx.colors().clone();
        let text = match self.summary {
            Some(summary) if summary.flows > 0 => {
                let mut text = format!(
                    "{} in the open Network Flows view",
                    widgets::plural(summary.flows, "flow", "flows")
                );
                if summary.blocked > 0 {
                    text.push_str(&format!(" · {} blocked", widgets::count(summary.blocked)));
                }
                text
            }
            Some(_) => "No flows in the open Network Flows view.".into(),
            None => "See this object's traffic, the policies that allow or block it and its peers."
                .into(),
        };
        let blocked = self.summary.is_some_and(|s| s.blocked > 0);
        let cluster = self.cluster.clone();
        let query = self.query.clone();
        v_flex()
            .px(u(14.0))
            .py(u(12.0))
            .gap(u(8.0))
            .border_b_1()
            .border_color(colors.border_variant)
            .child(
                div()
                    .text_size(u(11.0))
                    .font_weight(FontWeight::SEMIBOLD)
                    .text_color(colors.text_dim)
                    .child("NETWORK FLOWS"),
            )
            .child(
                div()
                    .text_size(u(12.0))
                    .text_color(if blocked {
                        colors.red
                    } else {
                        colors.text_muted
                    })
                    .child(text),
            )
            .child(h_flex().child(widgets::button(
                "details-show-flows",
                Some(IconName::Waypoints),
                "Show network flows",
                false,
                &colors,
                move |_, window, cx| {
                    view::open(
                        &cluster,
                        None,
                        Pending {
                            tab: Some(Tab::Flows),
                            query: Some(query.clone()),
                        },
                        window,
                        cx,
                    )
                },
            )))
    }
}
