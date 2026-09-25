//! Metrics: Prometheus discovery and PromQL client, metrics-server fallback.
//!
//! - [`service::MetricsService`]: the app-wide, demand-driven cache (current pod/node usage,
//!   pod histories, chart range queries), one entry per cluster. Read it from views; reads mark
//!   the data as wanted and it's refreshed in the background.
//! - [`provider::ServiceProvider`]: installed as `kubyl_resources`' `MetricsProvider`, so the
//!   Pods/Nodes CPU and MEMORY columns and the details dock show usage.
//! - [`discover`]: finds Prometheus Services (kube-prometheus-stack, prometheus-operated,
//!   OpenShift thanos-querier, VictoriaMetrics…) and probes them through the API server's
//!   service proxy.
//! - [`prometheus`]: the HTTP API client (service proxy or an external URL).
//! - [`openshift`]: monitoring behind an auth proxy (OpenShift): its Route, called with the
//!   user's token or a short-lived service-account token.
//! - [`queries`]: the versioned PromQL library, recording-rule aware, overridable in settings.
//! - [`metrics_server`]: `metrics.k8s.io` (current values only).
//! - [`panels`] and [`details`]: the Metrics section of the details (network, disk, throttling,
//!   pressure, OOM kills… for pods, nodes, namespaces, workloads and PVCs).
//! - [`settings`]: the `"metrics"` settings.json section.

pub mod details;
pub mod discover;
pub mod metrics_server;
pub mod openshift;
pub mod panels;
pub mod prometheus;
pub mod provider;
pub mod queries;
pub mod service;
pub mod settings;
mod status;

use gpui::{App, Window, actions};
use kubyl_core::{
    ActionRegistry, ActionSpec, ActiveContext, ChromeRegistry, Notification, NotificationCenter,
};
use kubyl_resources::metrics::Metrics;
use kubyl_settings::Settings;
use secrecy::SecretString;

pub use service::{MetricsService, RangeKey, RangeResult, RangeState, Source};

actions!(
    metrics,
    [
        /// Looks for Prometheus (and metrics-server) in the active cluster again.
        Redetect,
        /// Stores the Authorization header for the active cluster's external Prometheus URL in
        /// the OS keychain.
        SetPrometheusAuthorization,
        /// Removes that header from the keychain.
        ClearPrometheusAuthorization,
    ]
);

/// Registers settings, installs the service and the provider, the status bar item and actions.
///
/// Must run after `kubyl_kube::init` and `kubyl_resources::init`.
pub fn init(cx: &mut App) {
    Settings::register::<settings::MetricsSettings>(cx);
    let service = MetricsService::install(true, cx);
    Metrics::set_provider(cx, provider::ServiceProvider(service));
    Metrics::changed(cx);
    ChromeRegistry::add_status_item(cx, status::MetricsStatusItem);
    ChromeRegistry::add_details_section(cx, details::MetricsDetails);

    for spec in [
        ActionSpec::new("Metrics: Look for Prometheus Again", Redetect),
        ActionSpec::new(
            "Metrics: Set Prometheus Authorization Header…",
            SetPrometheusAuthorization,
        ),
        ActionSpec::new(
            "Metrics: Clear Prometheus Authorization Header",
            ClearPrometheusAuthorization,
        ),
    ] {
        ActionRegistry::register(cx, spec);
    }
    cx.on_action(|_: &Redetect, cx| {
        let (Some(cluster), Some(service)) = (active_cluster(cx), MetricsService::global(cx))
        else {
            return;
        };
        service.update(cx, |service, cx| service.redetect(&cluster, cx));
    });
    cx.on_action(|_: &SetPrometheusAuthorization, cx| {
        let Some(cluster) = active_cluster(cx) else {
            return;
        };
        // Global handlers run inside the dispatching window's update.
        cx.defer(move |cx| {
            let Some(window) = cx.active_window() else {
                return;
            };
            window
                .update(cx, |_, window: &mut Window, cx| {
                    kubyl_explorer::dialogs::prompt_secret(
                        "Prometheus Authorization header".into(),
                        "Header value, e.g. Bearer <token> (kept in the OS keychain)",
                        move |value, _, cx| {
                            let header = (!value.is_empty()).then(|| SecretString::from(value));
                            save_authorization(cluster.clone(), header, cx);
                        },
                        window,
                        cx,
                    )
                })
                .ok();
        });
    });
    cx.on_action(|_: &ClearPrometheusAuthorization, cx| {
        if let Some(cluster) = active_cluster(cx) {
            save_authorization(cluster, None, cx);
        }
    });
}

fn active_cluster(cx: &App) -> Option<kubyl_core::ClusterId> {
    ActiveContext::global(cx)
        .cluster
        .as_ref()
        .map(|c| c.id.clone())
}

fn save_authorization(cluster: kubyl_core::ClusterId, header: Option<SecretString>, cx: &mut App) {
    let clearing = header.is_none();
    let task = service::store_auth_header(cluster.clone(), header, cx);
    cx.spawn(async move |cx| {
        let result = task.await;
        cx.update(|cx| match result {
            Ok(()) => {
                NotificationCenter::push(
                    cx,
                    Notification::success(if clearing {
                        "Prometheus Authorization header removed."
                    } else {
                        "Prometheus Authorization header saved in the keychain."
                    }),
                );
                if let Some(service) = MetricsService::global(cx) {
                    service.update(cx, |service, cx| service.redetect(&cluster, cx));
                }
            }
            Err(err) => NotificationCenter::push(
                cx,
                Notification::error(format!("Couldn't store the header: {err}")),
            ),
        });
    })
    .detach();
}
