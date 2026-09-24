//! Background watches every connected cluster runs: namespaces and CRDs.
//!
//! Both run as futures on the Tokio runtime (via `spawn_kube`) and report through a channel;
//! [`crate::ConnectionManager`] batches the updates into its entity.

use std::collections::BTreeSet;

use futures::channel::mpsc::UnboundedSender;
use futures::{StreamExt as _, TryStreamExt as _};
use k8s_openapi::api::core::v1::Namespace;
use k8s_openapi::apiextensions_apiserver::pkg::apis::apiextensions::v1::CustomResourceDefinition;
use kube::core::PartialObjectMeta;
use kube::runtime::WatchStreamExt as _;
use kube::runtime::watcher::{self, Event, watcher};
use kube::{Api, Client};

/// What the namespace watch reports.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum NamespaceUpdate {
    /// The full, sorted list.
    Names(Vec<String>),
    /// Listing namespaces is forbidden; use fallbacks.
    Forbidden,
}

fn is_forbidden(err: &watcher::Error) -> bool {
    match err {
        watcher::Error::InitialListFailed(kube::Error::Api(status))
        | watcher::Error::WatchStartFailed(kube::Error::Api(status)) => status.code == 403,
        watcher::Error::WatchError(status) => status.code == 403,
        _ => false,
    }
}

/// Watches namespaces until the receiver goes away or listing is forbidden.
pub async fn watch_namespaces(client: Client, tx: UnboundedSender<NamespaceUpdate>) {
    let api: Api<PartialObjectMeta<Namespace>> = Api::all(client);
    let mut names = BTreeSet::new();
    let mut initializing = BTreeSet::new();
    let mut stream = watcher(api, watcher::Config::default())
        .default_backoff()
        .boxed();
    loop {
        let event = match stream.try_next().await {
            Ok(Some(event)) => event,
            Ok(None) => break,
            Err(err) if is_forbidden(&err) => {
                tx.unbounded_send(NamespaceUpdate::Forbidden).ok();
                return;
            }
            Err(err) => {
                tracing::debug!("namespace watch error: {err}");
                continue;
            }
        };
        let changed = match event {
            Event::Init => {
                initializing.clear();
                false
            }
            Event::InitApply(ns) => {
                initializing.insert(ns.metadata.name.unwrap_or_default());
                false
            }
            Event::InitDone => {
                names = std::mem::take(&mut initializing);
                true
            }
            Event::Apply(ns) => names.insert(ns.metadata.name.unwrap_or_default()),
            Event::Delete(ns) => names.remove(&ns.metadata.name.unwrap_or_default()),
        };
        if changed
            && tx
                .unbounded_send(NamespaceUpdate::Names(names.iter().cloned().collect()))
                .is_err()
        {
            return;
        }
    }
}

/// Sends the CRD count once the initial list is done, then again whenever a CRD is added,
/// removed or its spec changes (a new generation). Every message after the first means
/// "re-run discovery".
pub async fn watch_crds(client: Client, tx: UnboundedSender<usize>) {
    let api: Api<PartialObjectMeta<CustomResourceDefinition>> = Api::all(client);
    let mut stream = watcher(api, watcher::Config::default())
        .default_backoff()
        .boxed();
    let mut names = BTreeSet::new();
    let mut initializing = BTreeSet::new();
    let mut ready = false;
    while let Some(event) = stream.next().await {
        let event = match event {
            Ok(event) => event,
            Err(err) if is_forbidden(&err) => return,
            Err(err) => {
                tracing::debug!("CRD watch error: {err}");
                continue;
            }
        };
        let changed = match event {
            Event::Init => {
                initializing.clear();
                false
            }
            Event::InitApply(crd) => {
                initializing.insert(name_and_generation(&crd.metadata));
                false
            }
            Event::InitDone => {
                let changed = ready && initializing != names;
                names = std::mem::take(&mut initializing);
                if !ready {
                    ready = true;
                    tx.unbounded_send(names.len()).ok();
                }
                changed
            }
            Event::Apply(crd) => {
                let key = name_and_generation(&crd.metadata);
                let changed = !names.contains(&key);
                names.retain(|(name, _)| name != &key.0);
                names.insert(key);
                changed
            }
            Event::Delete(crd) => {
                let name = crd.metadata.name.clone().unwrap_or_default();
                names.retain(|(n, _)| n != &name);
                true
            }
        };
        if changed && tx.unbounded_send(names.len()).is_err() {
            return;
        }
    }
}

/// Status-only updates don't bump the generation, so they don't re-run discovery.
fn name_and_generation(meta: &kube::core::ObjectMeta) -> (String, i64) {
    (
        meta.name.clone().unwrap_or_default(),
        meta.generation.unwrap_or_default(),
    )
}
