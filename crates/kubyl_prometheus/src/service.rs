//! Which Prometheus-compatible servers each cluster has: Prometheus, Thanos Query, OpenShift's
//! thanos-querier, VictoriaMetrics. The Prometheus sidebar row shows while a cluster has at
//! least one, and the view lets the user pick between several.
//!
//! A server that wants a username and password (HTTP basic auth, a `401` with a `Basic`
//! challenge) is listed locked. The API server strips credentials from service-proxy requests,
//! so a signed-in server is read through a temporary loopback port-forward. The Authorization
//! header is kept in the keychain while the server takes it; once rejected, it's deleted and the
//! view asks again.
//!
//! Only discovery and sign-in live here ([`PrometheusCore`] in `kubyl_prometheus_core`, run in
//! an entity by [`PrometheusService`]). Data (targets, rules, query results) is read by the open view
//! itself, so nothing is polled for a view nobody has open.

use std::convert::Infallible;
use std::ops::Deref;
use std::sync::Arc;

use gpui::{App, AppContext as _, Context, Entity, EventEmitter, Global, Subscription};
use kubyl_core::ClusterId;
use kubyl_core::host::{Hosts, hosted};
use kubyl_kube::{ConnectionEvent, ConnectionManager};
use kubyl_metrics::MetricsService;
use kubyl_metrics::settings::MetricsSettings;
use kubyl_portforward::manager::PortForwardManager;
pub use kubyl_prometheus_core::servers::*;
pub use kubyl_prometheus_core::service::{
    ClusterConn, Conns, Credentials, Discovery, Keychain, MetricsInfo, PrometheusCore,
    PrometheusEffect,
};
use kubyl_settings::Settings;
use secrecy::SecretString;

/// The servers of every cluster. Reads go to the [`PrometheusCore`]; observe the entity to
/// re-render.
pub struct PrometheusService {
    core: PrometheusCore,
    _subscriptions: Vec<Subscription>,
}

impl Deref for PrometheusService {
    type Target = PrometheusCore;

    fn deref(&self) -> &PrometheusCore {
        &self.core
    }
}

impl EventEmitter<Infallible> for PrometheusService {}

impl Hosts<PrometheusCore> for PrometheusService {
    fn service(&mut self) -> &mut PrometheusCore {
        &mut self.core
    }

    fn apply(&mut self, effect: PrometheusEffect, cx: &mut Context<Self>) {
        match effect {
            PrometheusEffect::Changed => crate::changed(cx),
            PrometheusEffect::Tick => self.tick(cx),
        }
    }
}

struct GlobalService(Entity<PrometheusService>);

impl Global for GlobalService {}

impl PrometheusService {
    /// Creates the service and makes it global. `tick` starts the discovery loop (off in tests).
    pub fn install(tick: bool, cx: &mut App) -> Entity<Self> {
        let service = cx.new(Self::new);
        cx.set_global(GlobalService(service.clone()));
        if tick {
            service.update(cx, |this, cx| {
                hosted(this, cx, |core, host| core.start(host))
            });
        }
        service
    }

    pub fn global(cx: &App) -> Option<Entity<Self>> {
        cx.try_global::<GlobalService>().map(|g| g.0.clone())
    }

    fn new(cx: &mut Context<Self>) -> Self {
        let mut subscriptions = Vec::new();
        if let Some(manager) = ConnectionManager::try_global(cx) {
            subscriptions.push(cx.subscribe(&manager, |this, _, event, cx| {
                this.connection_event(event, cx)
            }));
        }
        Self {
            core: PrometheusCore::new(
                PortForwardManager::app_reach(cx),
                Arc::new(Keychain::default()),
            ),
            _subscriptions: subscriptions,
        }
    }

    // ----- Control -----

    /// Forgets what was found and looks again.
    pub fn redetect(&mut self, cluster: &ClusterId, cx: &mut Context<Self>) {
        let conns = Self::conns(cx);
        hosted(self, cx, |core, host| {
            core.redetect(cluster, conns.as_ref(), host)
        });
    }

    /// Signs in to a locked server with a username and password; they're kept in the keychain
    /// once the server takes them.
    pub fn sign_in(
        &mut self,
        cluster: &ClusterId,
        id: &str,
        username: &str,
        password: &SecretString,
        cx: &mut Context<Self>,
    ) {
        let conn = Self::conn(cluster, cx);
        hosted(self, cx, |core, host| {
            core.sign_in(cluster, id, username, password, conn, host)
        });
    }

    /// Signs in again with the credentials in the keychain.
    pub fn reconnect(&mut self, cluster: &ClusterId, id: &str, cx: &mut Context<Self>) {
        let conn = Self::conn(cluster, cx);
        hosted(self, cx, |core, host| {
            core.reconnect(cluster, id, conn, host)
        });
    }

    /// A read of a signed-in server was refused: its credentials no longer work. They're
    /// deleted, and the view asks again. `session` is the sign-in the read was made with: a
    /// late answer to older credentials changes nothing.
    pub fn rejected(
        &mut self,
        cluster: &ClusterId,
        id: &str,
        session: u64,
        cx: &mut Context<Self>,
    ) {
        let conn = Self::conn(cluster, cx);
        hosted(self, cx, |core, host| {
            core.rejected(cluster, id, session, &conn, host)
        });
    }

    fn connection_event(&mut self, event: &ConnectionEvent, cx: &mut Context<Self>) {
        match event {
            ConnectionEvent::StateChanged(id) => {
                let connected = ConnectionManager::try_global(cx)
                    .and_then(|m| m.read(cx).client(id))
                    .is_some();
                hosted(self, cx, |core, host| {
                    core.connection_changed(id, connected, host)
                });
            }
            ConnectionEvent::DiscoveryChanged(id) => {
                hosted(self, cx, |core, host| core.discovery_changed(id, host));
            }
            ConnectionEvent::Rekeyed { from, to } => {
                hosted(self, cx, |core, host| core.rekeyed(from, to, host));
            }
            _ => {}
        }
    }

    fn tick(&mut self, cx: &mut Context<Self>) {
        let conns = Self::conns(cx);
        hosted(self, cx, |core, host| core.tick(conns.as_ref(), host));
    }

    /// How to reach a cluster and where its servers' credentials may be in the keychain.
    fn conn(cluster: &ClusterId, cx: &App) -> ClusterConn {
        let manager = ConnectionManager::try_global(cx);
        ClusterConn {
            client: manager.as_ref().and_then(|m| m.read(cx).client(cluster)),
            settings_keys: manager
                .map(|m| m.read(cx).settings_keys(cluster))
                .unwrap_or_else(|| vec![cluster.to_string()]),
        }
    }

    /// The connected clusters with what their discovery reads, `None` without a connection
    /// manager.
    fn conns(cx: &App) -> Option<Conns> {
        let manager = ConnectionManager::try_global(cx)?;
        let manager = manager.read(cx);
        let metrics = MetricsService::global(cx);
        Some(
            manager
                .entries()
                .iter()
                .filter_map(|c| {
                    let client = manager.client(&c.id)?;
                    let settings_keys = manager.settings_keys(&c.id);
                    let disabled = Settings::get::<MetricsSettings>(cx)
                        .prometheus_for_keys(&settings_keys)
                        .is_some_and(|p| p.disabled);
                    // The metrics service knows settings overrides, external URLs and OpenShift
                    // Routes.
                    let metrics = match &metrics {
                        Some(metrics) => {
                            let metrics = metrics.read(cx);
                            MetricsInfo {
                                client: metrics.prometheus(&c.id),
                                pending: matches!(
                                    metrics.source(&c.id),
                                    kubyl_metrics::Source::Unknown
                                        | kubyl_metrics::Source::Detecting
                                ),
                            }
                        }
                        None => MetricsInfo::default(),
                    };
                    Some((
                        c.id.clone(),
                        Discovery {
                            conn: ClusterConn {
                                client: Some(client),
                                settings_keys,
                            },
                            disabled,
                            metrics,
                        },
                    ))
                })
                .collect(),
        )
    }
}
