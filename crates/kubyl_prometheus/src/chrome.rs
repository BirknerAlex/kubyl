//! The sidebar row under every cluster that has a Prometheus.

use std::sync::Arc;

use gpui::App;
use kubyl_core::{ClusterId, ViewKind};
use kubyl_explorer::catalog::{self, ViewRow};
use kubyl_settings::Settings;
use kubyl_ui::IconName;

use crate::service::PrometheusService;
use crate::settings::PrometheusSettings;
use crate::view;

pub(crate) fn init(cx: &mut App) {
    catalog::register_view_row(
        cx,
        ViewRow {
            id: "prometheus",
            after: "network_flows",
            label: "Prometheus",
            icon: IconName::Flame,
            kind: ViewKind::Custom(view::VIEW_KIND.into()),
            visible: Some(Arc::new(row_visible)),
            badge: None,
        },
    );
}

fn row_visible(cluster: &ClusterId, cx: &App) -> bool {
    Settings::get::<PrometheusSettings>(cx).sidebar
        && PrometheusService::global(cx)
            .is_some_and(|service| service.read(cx).has_instances(cluster) == Some(true))
}
