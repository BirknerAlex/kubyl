//! [`AlertsService`]: the app's alerts cache ([`AlertsCore`] from `kubyl_alerts_core`) in an
//! entity. The core polls what someone looks at (see its module docs); this adds what needs
//! GPUI: it hands the core a snapshot of the connections, the metrics service and the
//! notification settings once a second, shows its toasts, and tells the chrome when data changed.

use std::convert::Infallible;
use std::ops::Deref;

use gpui::{App, AppContext as _, Context, Entity, EventEmitter, Global, Subscription, Task};
use kubyl_core::host::{Hosts, hosted};
use kubyl_core::{ClusterId, Notification, NotificationCenter};
use kubyl_kube::{ConnectionEvent, ConnectionManager};
use kubyl_metrics::MetricsService;
use kubyl_portforward::manager::PortForwardManager;
use kubyl_settings::Settings;
use secrecy::SecretString;
use serde_json::Value;

pub use kubyl_alerts_core::cache::*;
pub use kubyl_alerts_core::service::{
    AlertsCore, AlertsEffect, ClusterAlerts, ClusterEnv, DiscoveryEnv, Env, MetricsEnv, NotifyEnv,
    Unlock, password_keys,
};

use crate::model::{Alert, AlertState};
use crate::settings::{AlertsSettings, NotifyClusters};

/// The app-wide alerts cache. Reads go to the [`AlertsCore`]; observe the entity to re-render.
pub struct AlertsService {
    core: AlertsCore,
    _subscriptions: Vec<Subscription>,
}

impl Deref for AlertsService {
    type Target = AlertsCore;

    fn deref(&self) -> &AlertsCore {
        &self.core
    }
}

impl EventEmitter<Infallible> for AlertsService {}

impl Hosts<AlertsCore> for AlertsService {
    fn service(&mut self) -> &mut AlertsCore {
        &mut self.core
    }

    fn apply(&mut self, effect: AlertsEffect, cx: &mut Context<Self>) {
        match effect {
            AlertsEffect::Changed => crate::changed(cx),
            AlertsEffect::Tick => self.tick(cx),
            AlertsEffect::Notify { cluster, notice } => {
                let show = cluster;
                let notification = Notification::from(notice).action("Show", move |window, cx| {
                    crate::view::open(&show, None, window, cx)
                });
                NotificationCenter::push(cx, notification);
            }
        }
    }
}

struct GlobalService(Entity<AlertsService>);

impl Global for GlobalService {}

impl AlertsService {
    /// Creates the service and makes it global. `tick` starts the refresh loop (off in tests).
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
        subscriptions.push(Settings::observe::<AlertsSettings>(
            cx,
            move |settings, cx| {
                let settings = settings.clone();
                weak.update(cx, |this, cx| {
                    hosted(this, cx, |core, host| core.settings_changed(settings, host));
                })
                .ok();
            },
        ));
        let reach = PortForwardManager::app_reach(cx);
        let mut this = Self {
            core: AlertsCore::new(Settings::get::<AlertsSettings>(cx).clone(), reach),
            _subscriptions: subscriptions,
        };
        if tick {
            hosted(&mut this, cx, |core, host| core.start(host));
        }
        this
    }

    // ----- Control -----

    /// Forgets the cluster's sources and looks again.
    pub fn redetect(&mut self, cluster: &ClusterId, cx: &mut Context<Self>) {
        hosted(self, cx, |core, host| core.redetect(cluster, host));
    }

    /// Reads the cluster again now (after a write, or "Refresh").
    pub fn refresh_now(&mut self, cluster: &ClusterId, cx: &mut Context<Self>) {
        hosted(self, cx, |core, host| core.refresh_now(cluster, host));
    }

    /// Signs in to a locked Alertmanager; the credentials are kept in the keychain once it
    /// takes them.
    pub fn sign_in(
        &mut self,
        cluster: &ClusterId,
        label: &str,
        username: &str,
        password: &SecretString,
        cx: &mut Context<Self>,
    ) {
        hosted(self, cx, |core, host| {
            core.sign_in(cluster, label, username, password, host)
        });
    }

    /// Signs in again with the credentials in the keychain.
    pub fn reconnect(&mut self, cluster: &ClusterId, label: &str, cx: &mut Context<Self>) {
        hosted(self, cx, |core, host| core.reconnect(cluster, label, host));
    }

    /// Creates (or with `id`, replaces) a silence on `source` (`None`: the first Alertmanager).
    /// The list updates right away and is read again after 2 s.
    pub fn post_silence(
        &mut self,
        cluster: &ClusterId,
        source: Option<&str>,
        body: Value,
        cx: &mut Context<Self>,
    ) -> Task<Result<String, String>> {
        let write = hosted(self, cx, |core, host| {
            core.post_silence(cluster, source, body, host)
        });
        cx.spawn(async move |_, _| write.await)
    }

    /// Expires a silence.
    pub fn expire_silence(
        &mut self,
        cluster: &ClusterId,
        source: &str,
        id: &str,
        cx: &mut Context<Self>,
    ) -> Task<Result<(), String>> {
        let write = hosted(self, cx, |core, host| {
            core.expire_silence(cluster, source, id, host)
        });
        cx.spawn(async move |_, _| write.await)
    }

    // ----- What the core reads from the app -----

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
            ConnectionEvent::ContextsChanged => {
                hosted(self, cx, |core, host| core.contexts_changed(host));
            }
            _ => {}
        }
    }

    /// The snapshot of the app the core's next tick reads, `None` without a connection manager.
    fn env(&self, cx: &App) -> Option<Env> {
        let manager = ConnectionManager::try_global(cx)?;
        let manager = manager.read(cx);
        let metrics = MetricsService::global(cx);
        let ready = self.core.notices_ready();
        let favorites = self.core.settings().notify.clusters == NotifyClusters::Favorites;
        let clusters = manager
            .entries()
            .iter()
            .filter_map(|c| {
                let client = manager.client(&c.id)?;
                let settings_keys = manager.settings_keys(&c.id);
                let discovery =
                    self.core
                        .discovery_due(&c.id, &settings_keys)
                        .then(|| DiscoveryEnv {
                            user_token: manager.bearer_token(&c.id),
                            metrics: metrics.as_ref().map(|metrics| {
                                let metrics = metrics.read(cx);
                                let source = metrics.source(&c.id);
                                MetricsEnv {
                                    prometheus: metrics.prometheus(&c.id),
                                    pending: matches!(
                                        source,
                                        kubyl_metrics::Source::Unknown
                                            | kubyl_metrics::Source::Detecting
                                    ),
                                }
                            }),
                        });
                let notify = ready.contains(&c.id).then(|| NotifyEnv {
                    display_name: Some(manager.display_name(&c.id).to_string()),
                    production: manager.caps(&c.id).production,
                    active: manager.active() == Some(&c.id),
                    favorite: favorites && is_favorite(&c.id, cx),
                });
                Some(ClusterEnv {
                    id: c.id.clone(),
                    client,
                    settings_keys,
                    discovery,
                    notify,
                })
            })
            .collect();
        Some(Env {
            window_active: cx.active_window().is_some(),
            clusters,
        })
    }

    fn tick(&mut self, cx: &mut Context<Self>) {
        let Some(env) = self.env(cx) else {
            return;
        };
        hosted(self, cx, |core, host| core.tick(&env, host));
    }

    /// Puts a cluster with an Alertmanager into the cache (tests of the views).
    #[cfg(test)]
    pub fn insert_for_test(
        &mut self,
        cluster: &ClusterId,
        alerts: Vec<Alert>,
        cx: &mut Context<Self>,
    ) {
        hosted(self, cx, |core, host| {
            core.insert_for_test(cluster, alerts, host)
        });
    }

    #[cfg(test)]
    pub(crate) fn lock_for_test(&mut self, cluster: &ClusterId, target: crate::discover::AmTarget) {
        self.core.lock_for_test(cluster, target);
    }

    #[cfg(test)]
    pub(crate) fn forget_demand_for_test(&mut self) {
        self.core.forget_demand_for_test();
    }
}

/// Whether `cluster` is one of the user's favorites (saved views don't make their cluster one).
fn is_favorite(cluster: &ClusterId, cx: &App) -> bool {
    kubyl_explorer::favorites::Favorites::global(cx)
        .read(cx)
        .items()
        .iter()
        .filter(|f| f.view.is_none())
        .any(|f| kubyl_explorer::favorites::cluster_of(f, cx).as_ref() == Some(cluster))
}

/// Per-cluster counts for chrome (badges, markers), without marking demand.
pub fn counts(cluster: &ClusterId, cx: &App) -> Option<Counts> {
    AlertsService::global(cx)?.read(cx).counts(cluster)
}

/// Firing alerts of `cluster`, most severe first, for tooltips and cards.
pub fn top_alerts(cluster: &ClusterId, n: usize, cx: &App) -> Vec<Alert> {
    let Some(service) = AlertsService::global(cx) else {
        return Vec::new();
    };
    let service = service.read(cx);
    let Some(state) = service.cluster(cluster, Pace::Background) else {
        return Vec::new();
    };
    state
        .alerts
        .iter()
        .filter(|a| matches!(a.state, AlertState::Firing | AlertState::Unprocessed))
        .take(n)
        .cloned()
        .collect()
}

/// The user a silence names as its creator: from `SelfSubjectReview`, else the kubeconfig user.
pub fn created_by(cluster: &ClusterId, cx: &App) -> (String, &'static str) {
    let Some(manager) = ConnectionManager::try_global(cx) else {
        return (String::new(), "");
    };
    let manager = manager.read(cx);
    if let Some(user) = manager
        .cluster(cluster)
        .and_then(|c| c.info.as_ref()?.user.clone())
        .filter(|u| !u.is_empty())
    {
        return (user, "from your sign-in");
    }
    match manager.context(cluster).and_then(|c| c.user.clone()) {
        Some(user) if !user.is_empty() => (user, "the kubeconfig user"),
        _ => (String::new(), ""),
    }
}

/// The active alerts of one object (the details section).
pub fn alerts_for(
    cluster: &ClusterId,
    object: &crate::view::rows::ObjectFilter,
    cx: &App,
) -> Vec<Alert> {
    let Some(service) = AlertsService::global(cx) else {
        return Vec::new();
    };
    let service = service.read(cx);
    let Some(state) = service.cluster(cluster, Pace::Background) else {
        return Vec::new();
    };
    state
        .alerts
        .iter()
        .filter(|a| {
            matches!(
                a.state,
                AlertState::Firing | AlertState::Pending | AlertState::Unprocessed
            )
        })
        .filter(|a| object.matches(a))
        .cloned()
        .collect()
}
