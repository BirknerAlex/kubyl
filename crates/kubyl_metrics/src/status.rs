//! Status bar item: the active cluster's metrics source (`Prometheus`, `metrics-server`).

use gpui::{
    AnyView, App, AppContext as _, Context, IntoElement, Render, SharedString, Subscription,
    Window, div, prelude::*,
};
use kubyl_core::actions::OpenView;
use kubyl_core::{ActiveContext, StatusBarItem, StatusBarPosition, ViewKind, ViewRequest};
use kubyl_ui::{ActiveColors, Icon, IconName, h_flex, u};

use crate::service::{MetricsService, Source};

pub struct MetricsStatusItem;

impl StatusBarItem for MetricsStatusItem {
    fn id(&self) -> &'static str {
        "metrics-source"
    }

    fn position(&self) -> StatusBarPosition {
        StatusBarPosition::Right
    }

    fn order(&self) -> i32 {
        -10
    }

    fn build(&self, _: &mut Window, cx: &mut App) -> AnyView {
        cx.new(MetricsStatusView::new).into()
    }
}

struct MetricsStatusView {
    _subscriptions: Vec<Subscription>,
}

impl MetricsStatusView {
    fn new(cx: &mut Context<Self>) -> Self {
        let mut subscriptions = vec![cx.observe_global::<ActiveContext>(|_, cx| cx.notify())];
        if let Some(service) = MetricsService::global(cx) {
            subscriptions.push(cx.observe(&service, |_, _, cx| cx.notify()));
        }
        Self {
            _subscriptions: subscriptions,
        }
    }
}

impl Render for MetricsStatusView {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let colors = cx.colors().clone();
        let cluster = ActiveContext::global(cx)
            .cluster
            .as_ref()
            .map(|c| c.id.clone());
        let source = match (&cluster, MetricsService::global(cx)) {
            (Some(cluster), Some(service)) => service.read(cx).source(cluster),
            _ => Source::Unknown,
        };
        let (color, tooltip): (_, SharedString) = match &source {
            Source::Prometheus { target } => (
                colors.green,
                format!("Prometheus · {}", target.label()).into(),
            ),
            Source::MetricsServer { note } => (
                colors.text_dim,
                format!("metrics-server (current values only). {note}").into(),
            ),
            Source::None { reason } => (colors.text_faint, reason.clone()),
            _ => (colors.text_dim, SharedString::default()),
        };
        let label = source.label();
        h_flex().id("metrics-source").gap(u(5.0)).when(
            !label.is_empty() && cluster.is_some(),
            |this| {
                let cluster = cluster.clone();
                this.cursor_pointer()
                    .child(Icon::new(IconName::Activity).size(12.0).color(color))
                    .child(div().child(label))
                    .when(!tooltip.is_empty(), |this| {
                        this.tooltip(move |window, cx| {
                            gpui_component::tooltip::Tooltip::new(tooltip.clone()).build(window, cx)
                        })
                    })
                    .on_click(move |_, window, cx| {
                        if let Some(cluster) = &cluster {
                            window.dispatch_action(
                                Box::new(OpenView(ViewRequest::for_resource(
                                    ViewKind::Overview,
                                    kubyl_core::ResourceRef::list(
                                        cluster.clone(),
                                        kubyl_core::Gvr::new("", "", ""),
                                        None,
                                    ),
                                ))),
                                cx,
                            );
                        }
                    })
            },
        )
    }
}
