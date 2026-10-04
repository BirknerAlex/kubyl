//! Metrics without the UI.
//!
//! - [`discover`], [`prometheus`], [`openshift`], [`transport`], [`basic_auth`]: finding a
//!   Prometheus (or Thanos, VictoriaMetrics, OpenShift monitoring) and querying it.
//! - [`metrics_server`]: the metrics.k8s.io fallback.
//! - [`queries`], [`panels`]: the PromQL behind usage columns and dashboard panels.
//! - [`service`]: [`MetricsCore`](service::MetricsCore), the demand-driven metrics cache.
//! - [`settings`]: the `"metrics"` settings.json section.
//!
//! `kubyl_metrics` re-exports these modules and adds the metrics service, the details panels and
//! the status bar item.

pub mod basic_auth;
pub mod discover;
pub mod metrics_server;
pub mod openshift;
pub mod panels;
pub mod prometheus;
pub mod queries;
pub mod service;
pub mod settings;
pub mod transport;
