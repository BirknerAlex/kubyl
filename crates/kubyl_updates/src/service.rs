//! [`Updates`]: the app-wide update state. Per cluster it detects the provider, reads it while
//! a view (or another crate) wants it (every 15 s, every 5 s while an update runs), runs
//! pre-flight checks and the writes that outlive a dialog.

use std::collections::HashMap;
use std::rc::Rc;
use std::sync::Arc;
use std::time::{Duration, Instant};

use gpui::{App, AppContext as _, Context, Entity, Global, Task};
use kubyl_core::{ClusterId, Notification, NotificationCenter, spawn_kube};
use kubyl_kube::{ConnectionEvent, ConnectionManager};
use kubyl_operators::helm::service::{Helm, HelmLease};

pub use kubyl_updates_core::service::*;

use crate::check::{self, Check};
use crate::detect::{self, Detected, Facts};
use crate::model::{Plan, ProviderKind, Status};
use crate::preflight::{self, HelmInput, Inputs, ReleaseRef};
use crate::provider::UpdateProvider;
use crate::settings::UpdatesSettings;

/// Keeps a cluster's reads going while held (views hold one).
#[derive(Clone)]
pub struct UpdatesLease(#[allow(dead_code)] Rc<()>);

struct ClusterUpdates {
    detected: Option<Detected>,
    /// The facts the provider was built from (a change rebuilds it).
    facts: Option<Facts>,
    provider: Option<Arc<dyn UpdateProvider>>,
    read: ReadState,
    last: Option<Arc<Status>>,
    fetched_at: Option<Instant>,
    reading: bool,
    /// Bumped when the provider is rebuilt; older reads are dropped.
    generation: u64,
    lease: Rc<()>,
    wanted_until: Instant,
    preflight: HashMap<String, Preflight>,
    busy: Option<String>,
    /// Which write `busy` belongs to (the state moves when the cluster is rekeyed, so a write
    /// finds it again by this and not by the id it started under).
    busy_token: u64,
    helm: Option<HelmLease>,
    /// Reads, pre-flight runs, the poll loop. Finished ones are pruned (a task must not drop
    /// itself).
    tasks: Vec<Task<()>>,
    /// The poll loop; `polling` says whether it still runs (it ends by itself and must not
    /// drop its own handle).
    poll: Option<Task<()>>,
    polling: bool,
}

impl ClusterUpdates {
    fn new(lease: Rc<()>) -> Self {
        Self {
            detected: None,
            facts: None,
            provider: None,
            read: ReadState::Loading,
            last: None,
            fetched_at: None,
            reading: false,
            generation: 0,
            lease,
            wanted_until: Instant::now() + KEEP,
            preflight: HashMap::new(),
            busy: None,
            busy_token: 0,
            helm: None,
            tasks: Vec::new(),
            poll: None,
            polling: false,
        }
    }

    fn status(&self) -> Option<&Arc<Status>> {
        match &self.read {
            ReadState::Ready(status) => Some(status),
            _ => self.last.as_ref(),
        }
    }
}

pub struct Updates {
    clusters: HashMap<ClusterId, ClusterUpdates>,
    /// Poll on timers (off in GPUI tests).
    poll: bool,
    /// The last write token handed out.
    writes: u64,
    _subscriptions: Vec<gpui::Subscription>,
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
                        let ids: Vec<ClusterId> = this.clusters.keys().cloned().collect();
                        for id in ids {
                            if let Some(state) = this.clusters.get_mut(&id) {
                                state.facts = None;
                            }
                            this.detect(&id, cx);
                        }
                    },
                ));
            }
            Self {
                clusters: HashMap::new(),
                poll,
                writes: 0,
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
            this.ensure(cluster, cx);
            UpdatesLease(this.clusters[cluster].lease.clone())
        }))
    }

    fn connection_event(&mut self, event: &ConnectionEvent, cx: &mut Context<Self>) {
        match event {
            ConnectionEvent::StateChanged(id) | ConnectionEvent::DiscoveryChanged(id) => {
                if self.clusters.contains_key(id) {
                    self.detect(id, cx);
                }
                cx.notify();
            }
            ConnectionEvent::Rekeyed { from, to } => {
                if let Some(mut state) = self.clusters.remove(from) {
                    // The provider holds the connection's client, which moved with it.
                    state.facts = None;
                    self.clusters.insert(to.clone(), state);
                    self.detect(to, cx);
                }
                cx.notify();
            }
            _ => {}
        }
    }

    /// Starts tracking `cluster` (detection, reads, the poll loop) and marks it wanted.
    fn ensure(&mut self, cluster: &ClusterId, cx: &mut Context<Self>) {
        if let Some(state) = self.clusters.get_mut(cluster) {
            state.wanted_until = Instant::now() + KEEP;
            if !state.polling && self.poll {
                self.start_poll(cluster, cx);
            }
            return;
        }
        self.clusters
            .insert(cluster.clone(), ClusterUpdates::new(Rc::new(())));
        self.detect(cluster, cx);
        if self.poll {
            self.start_poll(cluster, cx);
        }
    }

    fn start_poll(&mut self, cluster: &ClusterId, cx: &mut Context<Self>) {
        let id = cluster.clone();
        let task = cx.spawn(async move |this, cx| {
            loop {
                let delay = this
                    .update(cx, |this, cx| {
                        let state = this.clusters.get_mut(&id)?;
                        let wanted = Rc::strong_count(&state.lease) > 1
                            || Instant::now() < state.wanted_until;
                        if !wanted {
                            // Nobody looks: stop, and drop the Helm watches.
                            state.polling = false;
                            state.helm = None;
                            return None;
                        }
                        let updating = state.status().is_some_and(|s| s.updating());
                        this.read(&id, cx);
                        Some(if updating { UPDATING } else { IDLE })
                    })
                    .ok()
                    .flatten();
                let Some(delay) = delay else {
                    break;
                };
                cx.background_executor().timer(delay).await;
            }
        });
        if let Some(state) = self.clusters.get_mut(cluster) {
            // A previous loop has ended by now: replacing its handle is fine.
            state.poll = Some(task);
            state.polling = true;
        }
    }

    /// Detects (again) and rebuilds the provider when its inputs changed.
    fn detect(&mut self, cluster: &ClusterId, cx: &mut Context<Self>) {
        let manager = ConnectionManager::try_global(cx);
        let client = manager.as_ref().and_then(|m| m.read(cx).client(cluster));
        let facts = manager.as_ref().and_then(|m| facts(m.read(cx), cluster));
        let Some(state) = self.clusters.get_mut(cluster) else {
            return;
        };
        let (Some(client), Some(facts)) = (client, facts) else {
            // Disconnected or not discovered yet: nothing to read.
            state.detected = None;
            state.facts = None;
            state.provider = None;
            state.read = ReadState::Loading;
            state.generation += 1;
            cx.notify();
            return;
        };
        if state.facts.as_ref() == Some(&facts) && state.provider.is_some() {
            return;
        }
        let overridden = ConnectionManager::try_global(cx).and_then(|m| {
            kubyl_settings::Settings::get::<UpdatesSettings>(cx)
                .for_keys(&m.read(cx).settings_keys(cluster))
                .provider
        });
        let detected = match overridden {
            Some(setting) => Detected {
                kind: setting.kind(),
                reason: "picked in settings (updates.clusters.<cluster>.provider)".into(),
            },
            None => detect::detect(&facts),
        };
        let settings = kubyl_settings::Settings::get::<UpdatesSettings>(cx);
        let provider = build(
            manager.as_ref().map(|m| &**m.read(cx)),
            settings,
            cluster,
            &detected,
            &facts,
            client,
        );
        let Some(state) = self.clusters.get_mut(cluster) else {
            return;
        };
        let changed = state.detected.as_ref().map(|d| d.kind) != Some(detected.kind);
        state.detected = Some(detected);
        state.facts = Some(facts);
        state.provider = Some(provider);
        state.generation += 1;
        state.reading = false;
        if changed {
            state.read = ReadState::Loading;
            state.last = None;
            state.preflight.clear();
        }
        self.read(cluster, cx);
    }

    /// Reads the provider now (unless a read runs).
    pub fn read(&mut self, cluster: &ClusterId, cx: &mut Context<Self>) {
        let Some(state) = self.clusters.get_mut(cluster) else {
            return;
        };
        state.tasks.retain(|t| !t.is_ready());
        let Some(provider) = state.provider.clone() else {
            return;
        };
        if state.reading {
            return;
        }
        state.reading = true;
        let generation = state.generation;
        let manager = ConnectionManager::try_global(cx);
        let fallback = fallback_for(manager.as_ref().map(|m| &**m.read(cx)), cluster, &provider);
        let work = spawn_kube(cx, async move {
            match provider.read().await {
                Ok(status) => Ok(status),
                // A cloud without credentials still shows the Kubernetes side.
                Err(err) if fallback.is_some() && cloud_unavailable(&err) => {
                    let (fallback, note) = fallback.expect("checked");
                    match fallback.read().await {
                        Ok(mut status) => {
                            status.notes.insert(0, note(&err));
                            Ok(status)
                        }
                        Err(_) => Err(err),
                    }
                }
                Err(err) => Err(err),
            }
        });
        let id = cluster.clone();
        let task = cx.spawn(async move |this, cx| {
            let result = work.await;
            this.update(cx, |this, cx| {
                let Some(state) = this.clusters.get_mut(&id) else {
                    return;
                };
                if state.generation != generation {
                    return;
                }
                state.reading = false;
                state.fetched_at = Some(Instant::now());
                state.read = match result {
                    Ok(status) => {
                        let status = Arc::new(status);
                        state.last = Some(status.clone());
                        ReadState::Ready(status)
                    }
                    Err(err) => ReadState::Failed(err),
                };
                cx.notify();
            })
            .ok();
        });
        if let Some(state) = self.clusters.get_mut(cluster) {
            state.tasks.push(task);
        }
    }

    /// What a view shows for `cluster`.
    pub fn state(&self, cluster: &ClusterId, cx: &App) -> UpdateState {
        let connected =
            ConnectionManager::try_global(cx).is_some_and(|m| m.read(cx).client(cluster).is_some());
        if !connected {
            return UpdateState::NotConnected;
        }
        let Some(state) = self.clusters.get(cluster) else {
            return UpdateState::Detecting;
        };
        let Some(detected) = state.detected.clone() else {
            return UpdateState::Detecting;
        };
        UpdateState::Known {
            detected,
            read: state.read.clone(),
            last: state.last.clone(),
            fetched_at: state.fetched_at,
        }
    }

    /// The last good status of `cluster` (for other crates: badges, the title bar).
    pub fn status(&self, cluster: &ClusterId) -> Option<Arc<Status>> {
        self.clusters.get(cluster)?.status().cloned()
    }

    pub fn provider(&self, cluster: &ClusterId) -> Option<Arc<dyn UpdateProvider>> {
        self.clusters.get(cluster)?.provider.clone()
    }

    pub fn is_reading(&self, cluster: &ClusterId) -> bool {
        self.clusters.get(cluster).is_some_and(|s| s.reading)
    }

    /// The write in flight, if any.
    pub fn busy(&self, cluster: &ClusterId) -> Option<&str> {
        self.clusters.get(cluster)?.busy.as_deref()
    }

    /// The pre-flight run of `target`, if one ran against the current version.
    pub fn preflight(&self, cluster: &ClusterId, target: &str) -> Option<&Preflight> {
        let state = self.clusters.get(cluster)?;
        let current = state.status()?.current.version.clone();
        state
            .preflight
            .get(target)
            .filter(|run| run.current == current)
    }

    /// Every check of `target`: the run's plus the operators check, computed now from phase
    /// 12's live state (so it follows installs and removals without a re-run).
    pub fn checks(&self, cluster: &ClusterId, target: &str, cx: &mut App) -> Option<Vec<Check>> {
        let run = self.preflight(cluster, target)?.clone();
        let kind = self.clusters.get(cluster)?.detected.as_ref()?.kind;
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
        let Some(state) = self.clusters.get_mut(cluster) else {
            return;
        };
        state.tasks.retain(|t| !t.is_ready());
        let (Some(provider), Some(status), Some(detected)) = (
            state.provider.clone(),
            state.status().cloned(),
            state.detected.clone(),
        ) else {
            return;
        };
        if state.helm.is_none() {
            state.helm = Helm::watch(cluster, cx);
        }
        let current = status.current.version.clone();
        state.preflight.insert(
            target.to_string(),
            Preflight {
                target: target.to_string(),
                current: current.clone(),
                started: Instant::now(),
                finished: None,
                checks: Vec::new(),
            },
        );
        cx.notify();
        let id = cluster.clone();
        let target = target.to_string();
        let task = cx.spawn(async move |this, cx| {
            // The Helm list must have listed, or its check would pass on nothing; and phase 07
            // must have looked for Prometheus, or deprecated APIs come from /metrics only.
            let deadline = Instant::now() + HELM_WAIT;
            loop {
                let ready = cx.update(|cx| {
                    let helm = Helm::global(cx)
                        .and_then(|h| h.read(cx).snapshot(&id, cx))
                        .is_none_or(|s| !s.loading);
                    let metrics = kubyl_metrics::MetricsService::global(cx).is_none_or(|m| {
                        !matches!(
                            m.read(cx).source(&id),
                            kubyl_metrics::Source::Unknown | kubyl_metrics::Source::Detecting
                        )
                    });
                    helm && metrics
                });
                if ready || Instant::now() > deadline {
                    break;
                }
                cx.background_executor()
                    .timer(Duration::from_millis(300))
                    .await;
            }
            let work = cx.update(|cx| {
                let inputs = inputs(&id, detected.kind, &target, cx)?;
                let extras = provider.preflight_extras(&status, &target);
                Some(spawn_kube(cx, async move {
                    let (mut checks, extras) = futures::join!(preflight::run(inputs), extras);
                    checks.extend(extras);
                    checks
                }))
            });
            let checks = match work {
                Some(work) => work.await,
                // The cluster went away while waiting: finish the run, don't leave it
                // "running" forever.
                None => vec![Check::new(
                    "inputs",
                    "Pre-flight checks",
                    check::CheckStatus::Unknown,
                    "The cluster disconnected before the checks could run. Re-run them.",
                )],
            };
            this.update(cx, |this, cx| {
                if let Some(run) = this
                    .clusters
                    .get_mut(&id)
                    .and_then(|s| s.preflight.get_mut(&target))
                    .filter(|run| run.current == current)
                {
                    run.checks = checks;
                    run.finished = Some(Instant::now());
                }
                cx.notify();
            })
            .ok();
        });
        if let Some(state) = self.clusters.get_mut(cluster) {
            state.tasks.push(task);
        }
    }

    /// Runs a confirmed plan once. Refuses on read-only clusters; never retries.
    pub fn start(&mut self, cluster: &ClusterId, plan: Plan, cx: &mut Context<Self>) {
        let read_only =
            ConnectionManager::try_global(cx).is_some_and(|m| m.read(cx).caps(cluster).read_only);
        if read_only {
            NotificationCenter::push(
                cx,
                Notification::error("This cluster is read-only in Kubyl."),
            );
            return;
        }
        let Some(state) = self.clusters.get_mut(cluster) else {
            return;
        };
        let Some(provider) = state.provider.clone() else {
            return;
        };
        if state.busy.is_some() {
            return;
        }
        state.busy = Some(plan.title.clone());
        self.writes += 1;
        let token = self.writes;
        state.busy_token = token;
        cx.notify();
        let work = spawn_kube(cx, async move { provider.start(&plan).await });
        let task = cx.spawn(async move |this, cx| {
            let result = work.await;
            this.update(cx, |this, cx| {
                // The cluster may have been rekeyed meanwhile: `id` is then the old one.
                let current = finish_write(&mut this.clusters, token);
                match result {
                    Ok(message) => NotificationCenter::push(cx, Notification::info(message)),
                    Err(err) => NotificationCenter::push(cx, Notification::error(err.to_string())),
                }
                if let Some(current) = current {
                    this.read(&current, cx);
                }
                cx.notify();
            })
            .ok();
        });
        if let Some(state) = self.clusters.get_mut(cluster) {
            state.tasks.push(task);
        }
    }

    /// Puts a status in place without a provider (GPUI tests).
    #[cfg(test)]
    pub(crate) fn insert_for_test(
        &mut self,
        cluster: &ClusterId,
        detected: Detected,
        status: Status,
        cx: &mut Context<Self>,
    ) {
        let mut state = ClusterUpdates::new(Rc::new(()));
        state.detected = Some(detected);
        let status = Arc::new(status);
        state.last = Some(status.clone());
        state.read = ReadState::Ready(status);
        self.clusters.insert(cluster.clone(), state);
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
        let Some(state) = self.clusters.get_mut(cluster) else {
            return;
        };
        let Some(current) = state.status().map(|s| s.current.version.clone()) else {
            return;
        };
        state.preflight.insert(
            target.to_string(),
            Preflight {
                target: target.to_string(),
                current,
                started: Instant::now(),
                finished: checks.is_some().then(Instant::now),
                checks: checks.unwrap_or_default(),
            },
        );
        cx.notify();
    }

    /// Sets a cluster's provider without detection (GPUI tests).
    #[cfg(test)]
    pub(crate) fn set_provider_for_test(
        &mut self,
        cluster: &ClusterId,
        provider: Arc<dyn UpdateProvider>,
    ) {
        if let Some(state) = self.clusters.get_mut(cluster) {
            state.provider = Some(provider);
        }
    }
}

/// Clears the busy mark of write `token` wherever its cluster is now, and returns that id.
fn finish_write(
    clusters: &mut HashMap<ClusterId, ClusterUpdates>,
    token: u64,
) -> Option<ClusterId> {
    let (id, state) = clusters
        .iter_mut()
        .find(|(_, state)| state.busy_token == token)?;
    state.busy = None;
    state.busy_token = 0;
    Some(id.clone())
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
    use super::*;
    use crate::provider::ProviderError;
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

    /// A write that finishes after its cluster was rekeyed clears `busy` on the new id.
    #[test]
    fn a_write_finishing_after_a_rekey_clears_busy() {
        let mut state = ClusterUpdates::new(Rc::new(()));
        state.busy = Some("Update".into());
        state.busy_token = 7;
        let mut clusters = HashMap::new();
        // `Rekeyed` moved the state from the old id to the new one.
        clusters.insert(ClusterId::new("new-id"), state);
        clusters.insert(ClusterId::new("other"), ClusterUpdates::new(Rc::new(())));
        assert_eq!(
            finish_write(&mut clusters, 7),
            Some(ClusterId::new("new-id"))
        );
        assert!(clusters[&ClusterId::new("new-id")].busy.is_none());
        // Nothing left to clear.
        assert_eq!(finish_write(&mut clusters, 7), None);
    }
}
