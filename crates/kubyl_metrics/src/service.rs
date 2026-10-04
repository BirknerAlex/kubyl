//! [`MetricsService`]: the app's metrics cache ([`MetricsCore`] from `kubyl_metrics_core`) in an
//! entity, refreshed once a second with the connections of the clusters views asked about.

use std::convert::Infallible;
use std::ops::Deref;
use std::time::Duration;

use gpui::{App, AppContext as _, Context, Entity, EventEmitter, Global, Subscription, Task};
use kubyl_core::ClusterId;
use kubyl_core::host::{Hosts, hosted};
use kubyl_kube::{ConnectionEvent, ConnectionManager};
pub use kubyl_metrics_core::service::*;
use kubyl_resources::metrics::Metrics;
use kubyl_settings::Settings;
use secrecy::SecretString;

use crate::settings::MetricsSettings;

/// The app-wide metrics cache. Reads go to the [`MetricsCore`]; observe the entity (or the
/// `Metrics` global) to re-render.
pub struct MetricsService {
    core: MetricsCore,
    _tick: Task<()>,
    _subscriptions: Vec<Subscription>,
}

impl Deref for MetricsService {
    type Target = MetricsCore;

    fn deref(&self) -> &MetricsCore {
        &self.core
    }
}

impl EventEmitter<Infallible> for MetricsService {}

impl Hosts<MetricsCore> for MetricsService {
    fn service(&mut self) -> &mut MetricsCore {
        &mut self.core
    }

    fn apply(&mut self, effect: MetricsEffect, cx: &mut Context<Self>) {
        match effect {
            MetricsEffect::Changed => Metrics::changed(cx),
            MetricsEffect::Tick => self.tick(cx),
        }
    }
}

struct GlobalService(Entity<MetricsService>);

impl Global for GlobalService {}

impl MetricsService {
    /// Creates the service and makes it global. `tick` starts the refresh loop (off in tests
    /// that must not wake timers).
    pub fn install(tick: bool, cx: &mut App) -> Entity<Self> {
        let service = cx.new(|cx| Self::new(tick, cx));
        cx.set_global(GlobalService(service.clone()));
        service
    }

    pub fn global(cx: &App) -> Option<Entity<Self>> {
        cx.try_global::<GlobalService>().map(|g| g.0.clone())
    }

    fn new(tick: bool, cx: &mut Context<Self>) -> Self {
        let mut subscriptions = Vec::new();
        if let Some(manager) = ConnectionManager::try_global(cx) {
            subscriptions.push(cx.subscribe(&manager, |this, _, event, cx| {
                this.connection_event(event, cx)
            }));
        }
        let weak = cx.weak_entity();
        subscriptions.push(Settings::observe::<MetricsSettings>(
            cx,
            move |settings, cx| {
                let settings = settings.clone();
                weak.update(cx, |this, cx| {
                    let conns = Self::conns(&this.core, cx);
                    hosted(this, cx, |core, host| {
                        core.settings_changed(settings, conns.as_ref(), host)
                    });
                })
                .ok();
            },
        ));
        let task = if tick {
            cx.spawn(async move |this, cx| {
                loop {
                    cx.background_executor().timer(Duration::from_secs(1)).await;
                    if this.update(cx, |this, cx| this.tick(cx)).is_err() {
                        break;
                    }
                }
            })
        } else {
            Task::ready(())
        };
        Self {
            core: MetricsCore::new(Settings::get::<MetricsSettings>(cx).clone()),
            _tick: task,
            _subscriptions: subscriptions,
        }
    }

    /// The connections of the clusters views asked about, `None` without a connection manager.
    fn conns(core: &MetricsCore, cx: &App) -> Option<Conns> {
        let manager = ConnectionManager::try_global(cx)?;
        let manager = manager.read(cx);
        Some(
            core.wanted()
                .into_iter()
                .filter_map(|id| {
                    let client = manager.client(&id)?;
                    let conn = ClusterConn {
                        client,
                        settings_keys: manager.settings_keys(&id),
                        metrics_server: manager.caps(&id).metrics_server,
                        user_token: manager.bearer_token(&id),
                    };
                    Some((id, conn))
                })
                .collect(),
        )
    }

    /// Forgets the cluster's source and data and looks again.
    pub fn redetect(&mut self, cluster: &ClusterId, cx: &mut Context<Self>) {
        // The cluster becomes wanted inside `redetect`; ask for its connection too.
        let mut conns = Self::conns(&self.core, cx);
        if let (Some(conns), Some(manager)) = (conns.as_mut(), ConnectionManager::try_global(cx)) {
            let manager = manager.read(cx);
            if let Some(client) = manager.client(cluster) {
                conns.entry(cluster.clone()).or_insert_with(|| ClusterConn {
                    client,
                    settings_keys: manager.settings_keys(cluster),
                    metrics_server: manager.caps(cluster).metrics_server,
                    user_token: manager.bearer_token(cluster),
                });
            }
        }
        hosted(self, cx, |core, host| {
            core.redetect(cluster, conns.as_ref(), host)
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
            _ => {}
        }
    }

    fn tick(&mut self, cx: &mut Context<Self>) {
        let conns = Self::conns(&self.core, cx);
        hosted(self, cx, |core, host| core.tick(conns.as_ref(), host));
    }
}

/// Settings overrides of queries, for tests and the overview's subtitles.
pub fn render_query(cx: &App, cluster: &ClusterId, id: &str) -> Option<String> {
    let service = MetricsService::global(cx)?;
    service.read(cx).queries(cluster).render(id, &[])
}

/// Stores (or clears, with `None`) the Authorization header for a cluster's external
/// Prometheus in the OS keychain, off the UI thread.
pub fn store_auth_header(
    cluster: ClusterId,
    header: Option<SecretString>,
    cx: &mut App,
) -> Task<Result<(), String>> {
    let key = auth_key(&cluster);
    cx.background_executor().spawn(async move {
        match header {
            Some(header) => kubyl_kube::auth::store::set(&key, &header),
            None => kubyl_kube::auth::store::delete(&key),
        }
    })
}
