//! [`FluxCore`]: Flux per cluster on any [`Host`]: which Flux kinds the cluster serves (from
//! `ClusterCaps`, so it follows installs and uninstalls live) and where the controllers run
//! ([`crate::detect`]). Detection runs when the CRDs appear and again while an install is still
//! coming up (CRDs first, controllers later).

use std::collections::HashMap;
use std::convert::Infallible;
use std::sync::Arc;
use std::time::Duration;

use futures::future::BoxFuture;
use kubyl_base::host::{Host, HostExt as _, Service, TaskHandle};
use kubyl_base::{ClusterId, FluxCaps};

use crate::detect::Install;

/// While Flux is being installed, detection finds no controllers, or some that aren't ready.
/// It runs again: 3 s doubling, 5 times.
pub const DETECT_RETRY: Duration = Duration::from_secs(3);
pub const DETECT_RETRIES: u32 = 5;

/// Runs detection for a connected cluster (built from its client by the app).
pub type Detector = Arc<dyn Fn() -> BoxFuture<'static, Result<Install, String>> + Send + Sync>;

/// Where detection of a cluster stands.
#[derive(Clone, Debug, Default, PartialEq)]
pub enum Detection {
    #[default]
    Idle,
    Running,
    Done(Arc<Install>),
    Failed(String),
}

/// What the service asks the app to do.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum FluxEffect {
    /// The served kinds or the version changed: the explorer's Flux group shows them.
    TreeGroupsChanged,
}

#[derive(Default)]
struct ClusterFlux {
    caps: FluxCaps,
    detector: Option<Detector>,
    detection: Detection,
    retries: u32,
    _task: Option<TaskHandle>,
    _retry: Option<TaskHandle>,
}

/// Flux state of every cluster.
pub struct FluxCore {
    clusters: HashMap<ClusterId, ClusterFlux>,
    /// The first retry's delay ([`DETECT_RETRY`]; shorter in tests).
    retry: Duration,
}

impl Default for FluxCore {
    fn default() -> Self {
        Self::with_retry(DETECT_RETRY)
    }
}

impl Service for FluxCore {
    type Event = Infallible;
    type Effect = FluxEffect;
}

impl FluxCore {
    pub fn new() -> Self {
        Self::default()
    }

    /// With `retry` as the first retry's delay (doubling from there).
    pub fn with_retry(retry: Duration) -> Self {
        Self {
            clusters: HashMap::new(),
            retry,
        }
    }

    /// The clusters the service has an entry for.
    pub fn clusters(&self) -> Vec<ClusterId> {
        self.clusters.keys().cloned().collect()
    }

    /// Which Flux kinds `cluster` serves, as last synced.
    pub fn caps(&self, cluster: &ClusterId) -> FluxCaps {
        self.clusters
            .get(cluster)
            .map(|c| c.caps)
            .unwrap_or_default()
    }

    pub fn detection(&self, cluster: &ClusterId) -> Detection {
        self.clusters
            .get(cluster)
            .map(|c| c.detection.clone())
            .unwrap_or_default()
    }

    /// The controllers found on `cluster`.
    pub fn install(&self, cluster: &ClusterId) -> Option<Arc<Install>> {
        match &self.clusters.get(cluster)?.detection {
            Detection::Done(install) => Some(install.clone()),
            _ => None,
        }
    }

    /// Follows a cluster's caps and connection (`detector` is `None` while disconnected):
    /// detects when the CRDs appear, forgets everything when they go.
    pub fn sync_cluster(
        &mut self,
        cluster: &ClusterId,
        caps: FluxCaps,
        detector: Option<Detector>,
        host: &mut dyn Host<Self>,
    ) {
        let entry = self.clusters.entry(cluster.clone()).or_default();
        let changed = entry.caps != caps;
        entry.caps = caps;
        let connected = detector.is_some();
        entry.detector = detector;
        if !connected || !caps.any() {
            let had = entry.detection != Detection::Idle;
            entry.detection = Detection::Idle;
            entry._task = None;
            entry._retry = None;
            entry.retries = 0;
            if had || changed {
                host.effect(FluxEffect::TreeGroupsChanged);
                host.notify();
            }
            return;
        }
        if changed {
            entry.detection = Detection::Idle;
            entry.retries = 0;
            // A retry timer of the old CRDs would start a second detection.
            entry._retry = None;
            host.effect(FluxEffect::TreeGroupsChanged);
            host.notify();
        }
        if entry.detection == Detection::Idle {
            self.detect(cluster, host);
        }
    }

    /// Detects again (the user asked).
    pub fn redetect(&mut self, cluster: &ClusterId, host: &mut dyn Host<Self>) {
        if let Some(entry) = self.clusters.get_mut(cluster) {
            entry.retries = 0;
        }
        self.detect(cluster, host);
    }

    fn detect(&mut self, cluster: &ClusterId, host: &mut dyn Host<Self>) {
        let Some(entry) = self.clusters.get_mut(cluster) else {
            return;
        };
        let Some(detector) = entry.detector.clone() else {
            return;
        };
        if !matches!(entry.detection, Detection::Done(_)) {
            entry.detection = Detection::Running;
        }
        let id = cluster.clone();
        let retry = self.retry;
        entry._task = Some(host.spawn(detector(), move |this, result, host| {
            let Some(entry) = this.clusters.get_mut(&id) else {
                return;
            };
            let unfinished = match &result {
                // Still coming up: none found yet, or some not ready (an install creates its
                // Deployments one by one).
                Ok(install) => {
                    (install.controllers.is_empty() && !install.forbidden)
                        || install.controllers.iter().any(|c| !c.is_ready())
                }
                Err(_) => true,
            };
            entry.detection = match result {
                Ok(install) => Detection::Done(Arc::new(install)),
                Err(err) => Detection::Failed(err),
            };
            if unfinished && entry.retries < DETECT_RETRIES {
                let delay = retry * 2u32.pow(entry.retries);
                entry.retries += 1;
                let retry_id = id.clone();
                entry._retry = Some(host.after(delay, move |this, host| {
                    if let Some(entry) = this.clusters.get_mut(&retry_id)
                        && let Some(retry) = entry._retry.take()
                    {
                        // The timer that runs this is finishing: let it.
                        retry.detach();
                    }
                    this.detect(&retry_id, host);
                }));
            }
            host.effect(FluxEffect::TreeGroupsChanged);
            host.notify();
        }));
        host.notify();
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::detect::Controller;
    use futures::FutureExt as _;
    use kubyl_base::host::TestHost;
    use std::sync::atomic::{AtomicUsize, Ordering};

    fn caps() -> FluxCaps {
        FluxCaps {
            kustomizations: true,
            sources: true,
            ..FluxCaps::default()
        }
    }

    /// Finds a ready controller from call `controllers_after` on; before that none, or (with
    /// `not_ready`) one that isn't ready yet.
    fn detector_with(
        calls: Arc<AtomicUsize>,
        controllers_after: usize,
        not_ready: bool,
    ) -> Detector {
        Arc::new(move || {
            let n = calls.fetch_add(1, Ordering::SeqCst);
            async move {
                let controller = |ready| Controller {
                    name: "kustomize-controller".into(),
                    namespace: "flux-system".into(),
                    image: "ghcr.io/fluxcd/kustomize-controller:v1.9.6".into(),
                    version: Some("v1.9.6".into()),
                    ready,
                    replicas: 1,
                };
                let controllers = if n >= controllers_after {
                    vec![controller(1)]
                } else if not_ready {
                    vec![controller(0)]
                } else {
                    Vec::new()
                };
                Ok(Install {
                    namespace: Some("flux-system".into()),
                    controllers,
                    ..Install::default()
                })
            }
            .boxed()
        })
    }

    fn detector(calls: Arc<AtomicUsize>, controllers_after: usize) -> Detector {
        detector_with(calls, controllers_after, false)
    }

    #[test]
    fn detects_when_the_crds_appear_and_forgets_when_they_go() {
        let mut core = FluxCore::with_retry(Duration::from_millis(20));
        let mut host = TestHost::<FluxCore>::new();
        let cluster = ClusterId::new("kind");
        let calls = Arc::new(AtomicUsize::new(0));
        // No CRDs: nothing happens.
        core.sync_cluster(
            &cluster,
            FluxCaps::default(),
            Some(detector(calls.clone(), 0)),
            &mut host,
        );
        assert_eq!(core.detection(&cluster), Detection::Idle);
        assert_eq!(calls.load(Ordering::SeqCst), 0);
        // CRDs appear.
        core.sync_cluster(
            &cluster,
            caps(),
            Some(detector(calls.clone(), 0)),
            &mut host,
        );
        host.run_until_idle(&mut core);
        let install = core.install(&cluster).unwrap();
        assert_eq!(install.version().as_deref(), Some("v1.9.6"));
        assert!(host.effects.contains(&FluxEffect::TreeGroupsChanged));
        // They go.
        core.sync_cluster(
            &cluster,
            FluxCaps::default(),
            Some(detector(calls.clone(), 0)),
            &mut host,
        );
        assert_eq!(core.detection(&cluster), Detection::Idle);
        assert!(core.install(&cluster).is_none());
    }

    #[test]
    fn retries_while_the_controllers_come_up() {
        let mut core = FluxCore::with_retry(Duration::from_millis(20));
        let mut host = TestHost::<FluxCore>::new();
        let cluster = ClusterId::new("kind");
        let calls = Arc::new(AtomicUsize::new(0));
        core.sync_cluster(
            &cluster,
            caps(),
            Some(detector(calls.clone(), 1)),
            &mut host,
        );
        // The first run finds no controllers; the retry finds them.
        host.run_until(&mut core, |core, _| core.install(&cluster).is_some());
        assert!(core.install(&cluster).unwrap().controllers.is_empty());
        host.run_until_idle(&mut core);
        assert_eq!(calls.load(Ordering::SeqCst), 2);
        assert_eq!(core.install(&cluster).unwrap().controllers.len(), 1);
        // Disconnected: forgotten.
        core.sync_cluster(&cluster, caps(), None, &mut host);
        assert_eq!(core.detection(&cluster), Detection::Idle);
    }

    /// Gives up after [`DETECT_RETRIES`] retries; errors are retried; a forbidden listing
    /// isn't; "look again" runs detection again.
    #[test]
    fn retries_are_bounded_and_can_be_asked_for_again() {
        let fast = Duration::from_millis(5);
        let cluster = ClusterId::new("kind");
        // Never finds anything: the first run and DETECT_RETRIES retries.
        let mut core = FluxCore::with_retry(fast);
        let mut host = TestHost::<FluxCore>::new();
        let calls = Arc::new(AtomicUsize::new(0));
        core.sync_cluster(
            &cluster,
            caps(),
            Some(detector(calls.clone(), usize::MAX)),
            &mut host,
        );
        host.run_until_idle(&mut core);
        assert_eq!(calls.load(Ordering::SeqCst), 1 + DETECT_RETRIES as usize);
        assert!(core.install(&cluster).unwrap().controllers.is_empty());
        // Asked again: one more run.
        core.redetect(&cluster, &mut host);
        host.run_until(&mut core, |_, _| calls.load(Ordering::SeqCst) > 6);
        host.run_until_idle(&mut core);
        assert!(calls.load(Ordering::SeqCst) > 1 + DETECT_RETRIES as usize);

        // An error is retried, and the next run's result replaces it.
        let mut core = FluxCore::with_retry(fast);
        let mut host = TestHost::<FluxCore>::new();
        let errors = Arc::new(AtomicUsize::new(0));
        let failing: Detector = {
            let errors = errors.clone();
            Arc::new(move || {
                let n = errors.fetch_add(1, Ordering::SeqCst);
                async move {
                    if n == 0 {
                        Err("connection refused".to_string())
                    } else {
                        Ok(Install {
                            namespace: Some("flux-system".into()),
                            controllers: vec![Controller {
                                name: "kustomize-controller".into(),
                                namespace: "flux-system".into(),
                                image: String::new(),
                                version: None,
                                ready: 1,
                                replicas: 1,
                            }],
                            ..Install::default()
                        })
                    }
                }
                .boxed()
            })
        };
        core.sync_cluster(&cluster, caps(), Some(failing), &mut host);
        host.run_until(&mut core, |core, _| {
            matches!(core.detection(&cluster), Detection::Failed(_))
        });
        host.run_until_idle(&mut core);
        assert_eq!(errors.load(Ordering::SeqCst), 2);
        assert_eq!(core.install(&cluster).unwrap().controllers.len(), 1);

        // Not allowed to list Deployments: no point in retrying.
        let mut core = FluxCore::with_retry(fast);
        let mut host = TestHost::<FluxCore>::new();
        let forbidden_calls = Arc::new(AtomicUsize::new(0));
        let forbidden: Detector = {
            let calls = forbidden_calls.clone();
            Arc::new(move || {
                calls.fetch_add(1, Ordering::SeqCst);
                async move {
                    Ok(Install {
                        forbidden: true,
                        ..Install::default()
                    })
                }
                .boxed()
            })
        };
        core.sync_cluster(&cluster, caps(), Some(forbidden), &mut host);
        host.run_until_idle(&mut core);
        assert_eq!(forbidden_calls.load(Ordering::SeqCst), 1);
        assert!(core.install(&cluster).unwrap().forbidden);
    }

    #[test]
    fn retries_while_a_controller_isnt_ready() {
        let mut core = FluxCore::with_retry(Duration::from_millis(20));
        let mut host = TestHost::<FluxCore>::new();
        let cluster = ClusterId::new("kind");
        let calls = Arc::new(AtomicUsize::new(0));
        core.sync_cluster(
            &cluster,
            caps(),
            Some(detector_with(calls.clone(), 1, true)),
            &mut host,
        );
        host.run_until_idle(&mut core);
        assert_eq!(calls.load(Ordering::SeqCst), 2);
        assert!(core.install(&cluster).unwrap().controllers[0].is_ready());
    }
}
