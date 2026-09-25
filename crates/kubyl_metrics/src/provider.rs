//! The [`MetricsProvider`] behind the list columns and the details dock.

use gpui::{App, Entity};
use kubyl_core::ClusterId;
use kubyl_resources::metrics::{MetricsProvider, SourceStatus, Usage, UsageHistory};

use crate::service::{MetricsService, Source};

/// Answers from the [`MetricsService`] cache; asking keeps the data fresh.
pub struct ServiceProvider(pub Entity<MetricsService>);

impl MetricsProvider for ServiceProvider {
    fn pod_usage(
        &self,
        cluster: &ClusterId,
        namespace: &str,
        name: &str,
        cx: &App,
    ) -> Option<Usage> {
        self.0.read(cx).pod_usage(cluster, namespace, name)
    }

    fn node_usage(&self, cluster: &ClusterId, name: &str, cx: &App) -> Option<Usage> {
        self.0.read(cx).node_usage(cluster, name)
    }

    fn pod_history(
        &self,
        cluster: &ClusterId,
        namespace: &str,
        name: &str,
        cx: &App,
    ) -> Option<UsageHistory> {
        self.0.read(cx).pod_history(cluster, namespace, name)
    }

    fn source_status(&self, cluster: &ClusterId, cx: &App) -> SourceStatus {
        match self.0.read(cx).source(cluster) {
            Source::Unknown | Source::Detecting => SourceStatus::Detecting,
            source @ (Source::Prometheus { .. } | Source::MetricsServer { .. }) => {
                SourceStatus::Ready(source.label())
            }
            Source::None { reason } => SourceStatus::Unavailable(reason),
        }
    }
}
