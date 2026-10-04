//! [`FlowService`]: the app-wide, demand-driven flow state ([`FlowsCore`] from
//! `kubyl_netflow_core`) in an entity. Per cluster the core detects the backends, picks one
//! (settings may override), checks the permissions it needs, connects (a temporary loopback
//! forward for Hubble Relay and Whisker, the service proxy for the others) and runs one stream
//! per (cluster, server-side filter) while a view holds a [`FlowLease`]. This adds what needs
//! GPUI: it hands the core snapshots of the settings and connections, answers its question for
//! a cluster's Prometheus from the metrics service, and writes the backend menu's choice to
//! the settings.

use std::collections::HashMap;
use std::convert::Infallible;
use std::ops::Deref;
use std::time::Duration;

use gpui::{App, AppContext as _, Context, Entity, EventEmitter, Global};
use kubyl_core::ClusterId;
use kubyl_core::host::{Hosts, hosted};
use kubyl_kube::{ConnectionEvent, ConnectionManager};
use kubyl_portforward::manager::PortForwardManager;

pub use kubyl_netflow_core::service::*;

use crate::aggregate::Zoom;
use crate::detect::Served;
use crate::filter::FlowFilter;
use crate::provider::BackendKind;
use crate::settings::{BackendSetting, NetflowSettings};

/// The app-wide flow state. Reads go to the [`FlowsCore`]; observe the entity to re-render.
pub struct FlowService {
    core: FlowsCore,
    _subscriptions: Vec<gpui::Subscription>,
    /// GPUI tests have no connected clusters.
    #[cfg(test)]
    assume_connected: bool,
}

impl Deref for FlowService {
    type Target = FlowsCore;

    fn deref(&self) -> &FlowsCore {
        &self.core
    }
}

impl EventEmitter<Infallible> for FlowService {}

impl Hosts<FlowsCore> for FlowService {
    fn service(&mut self) -> &mut FlowsCore {
        &mut self.core
    }

    fn apply(&mut self, effect: FlowsEffect, cx: &mut Context<Self>) {
        match effect {
            FlowsEffect::Prometheus {
                cluster,
                generation,
            } => {
                let prometheus = kubyl_metrics::MetricsService::global(cx)
                    .and_then(|m| m.read(cx).prometheus(&cluster));
                hosted(self, cx, |core, host| {
                    core.prometheus_found(&cluster, generation, prometheus, host)
                });
            }
        }
    }
}

struct GlobalFlows(Entity<FlowService>);

impl Global for GlobalFlows {}

impl FlowService {
    /// Installs the global. `live`: timers and connections (off in GPUI tests).
    pub fn install(live: bool, cx: &mut App) -> Entity<Self> {
        let entity = cx.new(|cx: &mut Context<Self>| {
            let mut subscriptions = Vec::new();
            if let Some(manager) = ConnectionManager::try_global(cx) {
                subscriptions.push(cx.subscribe(&manager, |this: &mut Self, _, event, cx| {
                    this.connection_event(event, cx)
                }));
            }
            if cx.has_global::<kubyl_settings::Settings>() {
                subscriptions.push(cx.observe_global::<kubyl_settings::Settings>(
                    |this: &mut Self, cx| this.settings_changed(cx),
                ));
            }
            let mut this = Self {
                core: FlowsCore::new(live, PortForwardManager::app_reach(cx)),
                _subscriptions: subscriptions,
                #[cfg(test)]
                assume_connected: false,
            };
            hosted(&mut this, cx, |core, host| core.start(host));
            this
        });
        cx.set_global(GlobalFlows(entity.clone()));
        entity
    }

    pub fn global(cx: &App) -> Option<Entity<Self>> {
        cx.try_global::<GlobalFlows>().map(|g| g.0.clone())
    }

    // ----- Leases -----

    /// Keeps `cluster`'s stream with the server-side `filter` going (history from `window`
    /// back) while the lease lives. Starts detection and the backend when needed.
    pub fn lease(
        &mut self,
        cluster: &ClusterId,
        filter: &FlowFilter,
        window: Duration,
        cx: &mut Context<Self>,
    ) -> FlowLease {
        let env = self.env(Some(cluster), cx);
        hosted(self, cx, |core, host| {
            core.lease(cluster, filter, window, &env, host)
        })
    }

    /// Keeps `cluster` detected and its backend connected while the lease lives (a view before
    /// it knows its stream).
    pub fn watch(&mut self, cluster: &ClusterId, cx: &mut Context<Self>) -> FlowLease {
        let env = self.env(Some(cluster), cx);
        hosted(self, cx, |core, host| core.watch(cluster, &env, host))
    }

    /// Looks for backends again (the view's "Look again").
    pub fn redetect(&mut self, cluster: &ClusterId, cx: &mut Context<Self>) {
        let env = self.env(Some(cluster), cx);
        hosted(self, cx, |core, host| core.redetect(cluster, &env, host));
    }

    /// Connects again after a failure (the view's "Try again").
    pub fn reconnect(&mut self, cluster: &ClusterId, cx: &mut Context<Self>) {
        let env = self.env(Some(cluster), cx);
        hosted(self, cx, |core, host| core.reconnect(cluster, &env, host));
    }

    /// Writes `netflow.clusters.<cluster>.backend` (the header's backend menu).
    pub fn use_backend(cluster: &ClusterId, kind: BackendKind, cx: &mut App) {
        let keys = ConnectionManager::try_global(cx)
            .map(|m| m.read(cx).settings_keys(cluster))
            .unwrap_or_else(|| vec![cluster.to_string()]);
        let settings = settings(cx);
        // Where an override already is, else the context name.
        let key = keys
            .iter()
            .find(|k| settings.clusters.contains_key(*k))
            .or(keys.last())
            .cloned()
            .unwrap_or_else(|| cluster.to_string());
        kubyl_settings::Settings::update::<NetflowSettings>(cx, move |s| {
            s.clusters.entry(key).or_default().backend = Some(BackendSetting::of(kind));
        });
    }

    // ----- What the core reads from the app -----

    fn connection_event(&mut self, event: &ConnectionEvent, cx: &mut Context<Self>) {
        match event {
            ConnectionEvent::StateChanged(id) | ConnectionEvent::DiscoveryChanged(id) => {
                let env = self.env(Some(id), cx);
                hosted(self, cx, |core, host| {
                    core.connection_changed(id, &env, host)
                });
            }
            ConnectionEvent::Rekeyed { from, to } => {
                let env = self.env(Some(to), cx);
                hosted(self, cx, |core, host| core.rekeyed(from, to, &env, host));
            }
            _ => {}
        }
    }

    fn settings_changed(&mut self, cx: &mut Context<Self>) {
        let env = self.env(None, cx);
        hosted(self, cx, |core, host| core.settings_changed(&env, host));
    }

    /// The settings and the connections of the clusters the core follows, and `extra`.
    fn env(&self, extra: Option<&ClusterId>, cx: &App) -> Env {
        let manager = ConnectionManager::try_global(cx);
        let clusters = self
            .core
            .tracked()
            .into_iter()
            .chain(extra.cloned())
            .map(|id| {
                let env = match &manager {
                    Some(manager) => {
                        let manager = manager.read(cx);
                        let connection = (|| {
                            let client = manager.client(&id)?;
                            let cluster = manager.cluster(&id)?;
                            Some(Connection {
                                client,
                                served: cluster
                                    .discovery
                                    .as_ref()
                                    .map(|d| Served::from_discovery(d)),
                                git_version: cluster
                                    .info
                                    .as_ref()
                                    .map(|i| i.version.clone())
                                    .unwrap_or_default(),
                            })
                        })();
                        ClusterEnv {
                            settings_keys: manager.settings_keys(&id),
                            connection,
                        }
                    }
                    None => ClusterEnv {
                        settings_keys: vec![id.to_string()],
                        connection: None,
                    },
                };
                (id, env)
            })
            .collect::<HashMap<_, _>>();
        Env {
            settings: settings(cx),
            clusters,
        }
    }

    // ----- For views -----

    /// What a view shows for `cluster`.
    pub fn state(&self, cluster: &ClusterId, cx: &App) -> FlowState {
        let connected =
            ConnectionManager::try_global(cx).is_some_and(|m| m.read(cx).client(cluster).is_some());
        #[cfg(test)]
        let connected = connected || self.assume_connected;
        self.core.flow_state(cluster, connected)
    }

    /// The topology from the backend's metrics, fetched while asked for (every 30 s).
    pub fn metrics_graph(
        &mut self,
        cluster: &ClusterId,
        zoom: Zoom,
        window: Duration,
        filter: &FlowFilter,
        cx: &mut Context<Self>,
    ) -> Option<MetricsGraph> {
        hosted(self, cx, |core, host| {
            core.metrics_graph(cluster, zoom, window, filter, host)
        })
    }

    #[cfg(test)]
    pub(crate) fn tick_at(&mut self, now: std::time::Instant, cx: &mut Context<Self>) {
        hosted(self, cx, |core, host| core.tick_at(now, host));
    }

    /// Puts a ready backend and flows in place without a cluster (GPUI tests).
    #[cfg(test)]
    pub fn insert_for_test(
        &mut self,
        cluster: &ClusterId,
        provider: std::sync::Arc<dyn crate::provider::FlowProvider>,
        detection: crate::detect::Detection,
        cx: &mut Context<Self>,
    ) {
        hosted(self, cx, |core, host| {
            core.seed_ready(cluster, provider, detection, None, host)
        });
        self.assume_connected = true;
    }

    /// Adds flows to a stream (GPUI tests).
    #[cfg(test)]
    pub fn push_for_test(
        &mut self,
        cluster: &ClusterId,
        filter: &FlowFilter,
        flows: Vec<crate::model::Flow>,
        cx: &mut Context<Self>,
    ) {
        hosted(self, cx, |core, host| {
            core.push_flows(cluster, filter, flows, host)
        });
    }

    /// Whether a cluster's backend and forward are gone (GPUI tests).
    #[cfg(test)]
    pub fn idle_for_test(&self, cluster: &ClusterId) -> bool {
        self.core.is_idle(cluster)
    }
}

fn settings(cx: &App) -> NetflowSettings {
    if cx.has_global::<kubyl_settings::Settings>() {
        kubyl_settings::Settings::get::<NetflowSettings>(cx).clone()
    } else {
        NetflowSettings::default()
    }
}
