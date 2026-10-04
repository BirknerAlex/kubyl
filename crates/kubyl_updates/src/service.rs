//! [`Updates`]: the app-wide update state ([`UpdatesCore`] from `kubyl_updates_core`) in an
//! entity. The core detects the provider per cluster, reads it while a view (or another crate)
//! wants it (every 15 s, every 5 s while an update runs), runs pre-flight checks and the writes
//! that outlive a dialog. This adds what needs GPUI: it hands the core snapshots of the
//! connections, holds the Helm watches the checks read, and looks at the Helm and metrics
//! services for the pre-flight inputs.

use std::collections::HashMap;
use std::convert::Infallible;
use std::ops::Deref;

use gpui::{App, AppContext as _, Context, Entity, EventEmitter, Global};
use kubyl_core::ClusterId;
use kubyl_core::host::{Hosts, hosted};
use kubyl_kube::{ConnectionEvent, ConnectionManager};
use kubyl_operators::helm::service::{Helm, HelmLease};

pub use kubyl_updates_core::service::*;

use crate::check::{self, Check};
use crate::model::{Plan, ProviderKind};
use crate::preflight::{self, HelmInput, Inputs, ReleaseRef};
use crate::settings::UpdatesSettings;

/// The app-wide update state. Reads go to the [`UpdatesCore`]; observe the entity to re-render.
pub struct Updates {
    core: UpdatesCore,
    /// The Helm releases of clusters with pre-flight checks (they read them).
    helm: HashMap<ClusterId, HelmLease>,
    _subscriptions: Vec<gpui::Subscription>,
}

impl Deref for Updates {
    type Target = UpdatesCore;

    fn deref(&self) -> &UpdatesCore {
        &self.core
    }
}

impl EventEmitter<Infallible> for Updates {}

impl Hosts<UpdatesCore> for Updates {
    fn service(&mut self) -> &mut UpdatesCore {
        &mut self.core
    }

    fn apply(&mut self, effect: UpdatesEffect, cx: &mut Context<Self>) {
        match effect {
            UpdatesEffect::Read(cluster) => self.read(&cluster, cx),
            UpdatesEffect::WatchHelm(cluster) => {
                if !self.helm.contains_key(&cluster)
                    && let Some(lease) = Helm::watch(&cluster, cx)
                {
                    self.helm.insert(cluster, lease);
                }
            }
            UpdatesEffect::ReleaseHelm(cluster) => {
                self.helm.remove(&cluster);
            }
            UpdatesEffect::ProbePreflight {
                run,
                cluster,
                kind,
                target,
            } => {
                // The Helm list must have listed, or its check would pass on nothing; and
                // phase 07 must have looked for Prometheus, or deprecated APIs come from
                // /metrics only.
                let ready = preflight_ready(&cluster, cx);
                let probe = if ready || self.core.preflight_overdue(run) {
                    Probe::Go(inputs(&cluster, kind, &target, cx).map(Box::new))
                } else {
                    Probe::Waiting
                };
                hosted(self, cx, |core, host| {
                    core.preflight_probe(run, probe, host)
                });
            }
        }
    }
}

struct GlobalUpdates(Entity<Updates>);

impl Global for GlobalUpdates {}

impl Updates {
    /// Installs the global. `poll`: read on timers (off in GPUI tests).
    pub fn install(poll: bool, cx: &mut App) -> Entity<Self> {
        let entity = cx.new(|cx: &mut Context<Self>| {
            let mut subscriptions = Vec::new();
            if let Some(manager) = ConnectionManager::try_global(cx) {
                subscriptions.push(cx.subscribe(&manager, |this: &mut Self, _, event, cx| {
                    this.connection_event(event, cx)
                }));
            }
            if cx.has_global::<kubyl_settings::Settings>() {
                // A provider override or cloud settings changed: build the providers again.
                subscriptions.push(cx.observe_global::<kubyl_settings::Settings>(
                    |this: &mut Self, cx| {
                        let conns = conns(this.core.tracked(), cx);
                        hosted(this, cx, |core, host| core.settings_changed(&conns, host));
                    },
                ));
            }
            Self {
                core: UpdatesCore::new(poll),
                helm: HashMap::new(),
                _subscriptions: subscriptions,
            }
        });
        cx.set_global(GlobalUpdates(entity.clone()));
        entity
    }

    pub fn global(cx: &App) -> Option<Entity<Self>> {
        cx.try_global::<GlobalUpdates>().map(|g| g.0.clone())
    }

    /// Keeps `cluster`'s reads going while the returned lease lives.
    pub fn watch(cluster: &ClusterId, cx: &mut App) -> Option<UpdatesLease> {
        let updates = Self::global(cx)?;
        Some(updates.update(cx, |this, cx| {
            let conns = conns([cluster.clone()], cx);
            hosted(this, cx, |core, host| core.watch(cluster, &conns, host))
        }))
    }

    fn connection_event(&mut self, event: &ConnectionEvent, cx: &mut Context<Self>) {
        match event {
            ConnectionEvent::StateChanged(id) | ConnectionEvent::DiscoveryChanged(id) => {
                let conns = conns([id.clone()], cx);
                hosted(self, cx, |core, host| {
                    core.connection_changed(id, &conns, host)
                });
            }
            ConnectionEvent::Rekeyed { from, to } => {
                let conns = conns([to.clone()], cx);
                hosted(self, cx, |core, host| core.rekeyed(from, to, &conns, host));
            }
            _ => {}
        }
    }

    /// Reads the provider now (unless a read runs).
    pub fn read(&mut self, cluster: &ClusterId, cx: &mut Context<Self>) {
        let conns = conns([cluster.clone()], cx);
        hosted(self, cx, |core, host| core.read(cluster, &conns, host));
    }

    /// What a view shows for `cluster`.
    pub fn state(&self, cluster: &ClusterId, cx: &App) -> UpdateState {
        let connected =
            ConnectionManager::try_global(cx).is_some_and(|m| m.read(cx).client(cluster).is_some());
        if !connected {
            return UpdateState::NotConnected;
        }
        self.core.known(cluster)
    }

    /// Every check of `target`: the run's plus the operators check, computed now from phase
    /// 12's live state (so it follows installs and removals without a re-run).
    pub fn checks(&self, cluster: &ClusterId, target: &str, cx: &mut App) -> Option<Vec<Check>> {
        let run = self.core.preflight(cluster, target)?.clone();
        let kind = self.core.detected(cluster)?.kind;
        let installed = kubyl_operators::api::installed(cluster, cx);
        let openshift = (kind == ProviderKind::OpenShift)
            .then(|| crate::version::minor_of(target))
            .flatten();
        let mut checks = run.checks;
        checks.push(preflight::operators(
            &installed,
            preflight::target_kube_minor(kind, target),
            openshift,
        ));
        check::sort(&mut checks);
        Some(checks)
    }

    /// Runs the pre-flight checks for `target` (again).
    pub fn run_preflight(&mut self, cluster: &ClusterId, target: &str, cx: &mut Context<Self>) {
        hosted(self, cx, |core, host| {
            core.run_preflight(cluster, target, host)
        });
    }

    /// Runs a confirmed plan once. Refuses on read-only clusters; never retries.
    pub fn start(&mut self, cluster: &ClusterId, plan: Plan, cx: &mut Context<Self>) {
        let conns = conns([cluster.clone()], cx);
        hosted(self, cx, |core, host| {
            core.start(cluster, plan, &conns, host)
        });
    }

    /// Puts a status in place without a provider (GPUI tests).
    #[cfg(test)]
    pub(crate) fn insert_for_test(
        &mut self,
        cluster: &ClusterId,
        detected: crate::detect::Detected,
        status: crate::model::Status,
        cx: &mut Context<Self>,
    ) {
        self.core.seed(cluster, detected, status);
        cx.notify();
    }

    /// Sets a target's pre-flight run: running (`checks` None) or finished (GPUI tests).
    #[cfg(test)]
    pub(crate) fn set_preflight_for_test(
        &mut self,
        cluster: &ClusterId,
        target: &str,
        checks: Option<Vec<Check>>,
        cx: &mut Context<Self>,
    ) {
        self.core.seed_preflight(cluster, target, checks);
        cx.notify();
    }

    /// Sets a cluster's provider without detection (GPUI tests).
    #[cfg(test)]
    pub(crate) fn set_provider_for_test(
        &mut self,
        cluster: &ClusterId,
        provider: std::sync::Arc<dyn crate::provider::UpdateProvider>,
    ) {
        self.core.seed_provider(cluster, provider);
    }
}

/// The connections of `clusters` as the core sees them (empty without a connection manager).
fn conns(clusters: impl IntoIterator<Item = ClusterId>, cx: &App) -> Conns {
    let Some(manager) = ConnectionManager::try_global(cx) else {
        return Conns::new();
    };
    let manager = manager.read(cx);
    let settings = kubyl_settings::Settings::get::<UpdatesSettings>(cx);
    clusters
        .into_iter()
        .map(|id| {
            let conn = ClusterConn::from_manager(manager, &id, settings);
            (id, conn)
        })
        .collect()
}

/// Whether the Helm releases listed and metrics detection finished (or nothing runs them).
fn preflight_ready(cluster: &ClusterId, cx: &App) -> bool {
    let helm = Helm::global(cx)
        .and_then(|h| h.read(cx).snapshot(cluster, cx))
        .is_none_or(|s| !s.loading);
    let metrics = kubyl_metrics::MetricsService::global(cx).is_none_or(|m| {
        !matches!(
            m.read(cx).source(cluster),
            kubyl_metrics::Source::Unknown | kubyl_metrics::Source::Detecting
        )
    });
    helm && metrics
}

/// The inputs of a pre-flight run, from the connection, discovery, Prometheus and Helm.
fn inputs(cluster: &ClusterId, kind: ProviderKind, target: &str, cx: &mut App) -> Option<Inputs> {
    let manager = ConnectionManager::try_global(cx)?;
    let (client, discovery, current_kube) = {
        let manager = manager.read(cx);
        let connection = manager.cluster(cluster)?;
        (
            manager.client(cluster)?,
            connection.discovery.clone()?,
            connection.info.as_ref()?.version.clone(),
        )
    };
    let prometheus =
        kubyl_metrics::MetricsService::global(cx).and_then(|m| m.read(cx).prometheus(cluster));
    let helm = match Helm::global(cx).and_then(|h| h.read(cx).snapshot(cluster, cx)) {
        Some(snapshot) => match (&snapshot.problem, snapshot.releases.is_empty()) {
            (Some(problem), true) => HelmInput::Problem(problem.clone()),
            _ => HelmInput::Releases(
                snapshot
                    .releases
                    .iter()
                    .map(|row| ReleaseRef {
                        namespace: row.namespace.clone(),
                        name: row.name.clone(),
                        driver: row.driver,
                        object: row.latest().object.clone(),
                        revision: row.latest().revision,
                    })
                    .collect(),
            ),
        },
        None => HelmInput::Problem("the Helm releases couldn't be listed.".into()),
    };
    Some(Inputs {
        client,
        provider: kind,
        current_kube,
        target: target.to_string(),
        target_kube: preflight::target_kube_minor(kind, target),
        prometheus,
        api_request_counts: discovery.preferred().any(|r| {
            r.gvr.group == "apiserver.openshift.io" && r.gvr.resource == "apirequestcounts"
        }),
        scan: preflight::scan_kinds(&discovery),
        helm,
    })
}

#[cfg(test)]
mod tests {
    use std::sync::Arc;

    use super::*;
    use crate::detect::Detected;
    use crate::model::Status;
    use crate::provider::{ProviderError, UpdateProvider};
    use gpui::TestAppContext;

    struct Idle;

    impl UpdateProvider for Idle {
        fn kind(&self) -> ProviderKind {
            ProviderKind::OpenShift
        }

        fn read(&self) -> crate::provider::ProviderFuture<Result<Status, ProviderError>> {
            Box::pin(async { Ok(Status::default()) })
        }
    }

    /// A cluster that's gone before the checks start (no connection, no discovery) finishes
    /// the run with a "not checked" result instead of leaving it running forever.
    #[gpui::test]
    fn a_run_without_inputs_finishes(cx: &mut TestAppContext) {
        let dir = tempfile::tempdir().unwrap();
        let updates = cx.update(|cx| {
            kubyl_core::init(cx);
            kubyl_settings::init_with_dir(cx, dir.path());
            Updates::install(false, cx)
        });
        let cluster = ClusterId::new("gone");
        let mut status = Status::default();
        status.current.version = "4.17.8".into();
        updates.update(cx, |u, cx| {
            u.insert_for_test(
                &cluster,
                Detected {
                    kind: ProviderKind::OpenShift,
                    reason: "test".into(),
                },
                status,
                cx,
            );
            u.set_provider_for_test(&cluster, Arc::new(Idle));
            u.run_preflight(&cluster, "4.17.12", cx);
            assert!(u.preflight(&cluster, "4.17.12").unwrap().running());
        });
        cx.run_until_parked();
        updates.read_with(cx, |u, _| {
            let run = u.preflight(&cluster, "4.17.12").unwrap();
            assert!(!run.running());
            assert_eq!(run.checks.len(), 1);
            assert_eq!(run.checks[0].status, check::CheckStatus::Unknown);
        });
    }
}
