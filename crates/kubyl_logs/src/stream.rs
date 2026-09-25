//! Kube log streaming: single container, all containers of a pod, or all pods of a label
//! selector (workload), each with independent reconnect-with-backoff. Runs entirely off the UI
//! thread; the view drives it through [`run`] via `kubyl_core::spawn_kube` and consumes
//! [`StreamEvent`]s from the returned channel.
//!
//! Workload sources are membership-polled every [`POLL_INTERVAL`] rather than watched, to avoid
//! depending on the exact shape of `kube::runtime::watcher`'s event enum across kube releases;
//! see the phase 05 handoff log for the trade-off.

use std::collections::HashMap;
use std::time::Duration;

use futures::AsyncBufReadExt as _;
use futures::StreamExt as _;
use futures::channel::mpsc;
use k8s_openapi::api::core::v1::Pod;
use kube::api::{Api, ListParams, LogParams};

/// Where the lines of one log view come from.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum LogSource {
    /// One container of one pod.
    Container { pod: String, container: String },
    /// Every container of one pod.
    Pod { pod: String, init_containers: bool },
    /// Every pod matching a label selector (a workload's `spec.selector`, or a free-form one).
    Workload { label_selector: String },
}

#[derive(Clone, Debug, PartialEq)]
pub struct LogOptions {
    pub follow: bool,
    pub since_seconds: Option<i64>,
    pub tail_lines: Option<i64>,
    pub timestamps: bool,
    pub previous: bool,
}

impl Default for LogOptions {
    fn default() -> Self {
        Self {
            follow: true,
            since_seconds: None,
            tail_lines: Some(1000),
            timestamps: false,
            previous: false,
        }
    }
}

/// How often a workload source re-lists pods to notice new/deleted ones.
const POLL_INTERVAL: Duration = Duration::from_secs(4);
const BACKOFF_START: Duration = Duration::from_secs(1);
const BACKOFF_MAX: Duration = Duration::from_secs(30);

#[derive(Clone, Debug)]
pub enum StreamEvent {
    Line {
        pod: String,
        container: String,
        text: String,
    },
    PodJoined(String),
    PodGone(String),
    Reconnecting {
        pod: String,
        container: String,
    },
    Error(String),
}

/// Aborts its task when dropped, so cancelling the outer `spawn_kube` task (or a pod leaving a
/// workload's selector) stops every stream it started.
struct TaskGuard(Option<tokio::task::JoinHandle<()>>);

impl TaskGuard {
    fn new(handle: tokio::task::JoinHandle<()>) -> Self {
        Self(Some(handle))
    }

    /// Extracts the join handle without aborting the task, so the caller can await its natural
    /// completion (e.g. a non-follow container task that's expected to finish on its own).
    fn into_join_handle(mut self) -> tokio::task::JoinHandle<()> {
        self.0.take().expect("join handle taken twice")
    }
}

impl Drop for TaskGuard {
    fn drop(&mut self) {
        if let Some(handle) = self.0.take() {
            handle.abort();
        }
    }
}

/// Streams `source` until the returned future is dropped (or the receiver end of `tx` is
/// dropped). Never returns on its own when `options.follow` is set.
pub async fn run(
    client: kube::Client,
    namespace: String,
    source: LogSource,
    options: LogOptions,
    tx: mpsc::UnboundedSender<StreamEvent>,
) {
    match source {
        LogSource::Container { pod, container } => {
            let _guard = TaskGuard::new(tokio::spawn(container_task(
                client, namespace, pod, container, options, tx,
            )));
            std::future::pending::<()>().await;
        }
        LogSource::Pod {
            pod,
            init_containers,
        } => {
            let api: Api<Pod> = Api::namespaced(client.clone(), &namespace);
            let containers = match api.get(&pod).await {
                Ok(p) => container_names(&p, init_containers),
                Err(err) => {
                    tx.unbounded_send(StreamEvent::Error(err.to_string())).ok();
                    return;
                }
            };
            let mut guards = Vec::new();
            for container in containers {
                guards.push(TaskGuard::new(tokio::spawn(container_task(
                    client.clone(),
                    namespace.clone(),
                    pod.clone(),
                    container,
                    options.clone(),
                    tx.clone(),
                ))));
            }
            std::future::pending::<()>().await;
        }
        LogSource::Workload { label_selector } => {
            workload_loop(client, namespace, label_selector, options, tx).await;
        }
    }
}

fn container_names(pod: &Pod, init_containers: bool) -> Vec<String> {
    let spec = match &pod.spec {
        Some(spec) => spec,
        None => return Vec::new(),
    };
    let mut names: Vec<String> = spec.containers.iter().map(|c| c.name.clone()).collect();
    if init_containers && let Some(init) = &spec.init_containers {
        names.extend(init.iter().map(|c| c.name.clone()));
    }
    names
}

/// Re-lists pods matching `label_selector` every [`POLL_INTERVAL`], starting/stopping
/// per-container streams as pods join or leave.
async fn workload_loop(
    client: kube::Client,
    namespace: String,
    label_selector: String,
    options: LogOptions,
    tx: mpsc::UnboundedSender<StreamEvent>,
) {
    let api: Api<Pod> = Api::namespaced(client.clone(), &namespace);
    let params = ListParams::default().labels(&label_selector);
    let mut known: HashMap<String, Vec<TaskGuard>> = HashMap::new();
    loop {
        match api.list(&params).await {
            Ok(list) => {
                let mut seen = std::collections::HashSet::new();
                for pod in &list.items {
                    let Some(name) = pod.metadata.name.clone() else {
                        continue;
                    };
                    seen.insert(name.clone());
                    if known.contains_key(&name) {
                        continue;
                    }
                    tx.unbounded_send(StreamEvent::PodJoined(name.clone())).ok();
                    let mut guards = Vec::new();
                    for container in container_names(pod, false) {
                        guards.push(TaskGuard::new(tokio::spawn(container_task(
                            client.clone(),
                            namespace.clone(),
                            name.clone(),
                            container,
                            options.clone(),
                            tx.clone(),
                        ))));
                    }
                    known.insert(name, guards);
                }
                let gone: Vec<String> = known
                    .keys()
                    .filter(|name| !seen.contains(*name))
                    .cloned()
                    .collect();
                for name in gone {
                    known.remove(&name);
                    tx.unbounded_send(StreamEvent::PodGone(name)).ok();
                }
            }
            Err(err) => {
                tx.unbounded_send(StreamEvent::Error(err.to_string())).ok();
            }
        }
        if !options.follow {
            // A non-follow view: don't abort the per-container tasks we just started (dropping
            // `known`'s `TaskGuard`s would do that immediately, before they deliver any lines).
            // Wait for each to finish streaming its (already-bounded, non-follow) log instead.
            for guards in known.into_values() {
                for guard in guards {
                    let _ = guard.into_join_handle().await;
                }
            }
            return;
        }
        tokio::time::sleep(POLL_INTERVAL).await;
    }
}

/// Streams one container, splitting the byte stream on `\n` and reconnecting with backoff on
/// error (a gap-marker event is left for the view to render).
async fn container_task(
    client: kube::Client,
    namespace: String,
    pod: String,
    container: String,
    options: LogOptions,
    tx: mpsc::UnboundedSender<StreamEvent>,
) {
    let api: Api<Pod> = Api::namespaced(client, &namespace);
    let mut backoff = BACKOFF_START;
    let mut first_attempt = true;
    // Wall-clock time of the last line we delivered (or of task start, if none yet). Used to set
    // `since_seconds` on reconnect so a terminated/rotated container doesn't replay its whole log
    // on every retry (e.g. an infinite fast reconnect loop against a completed Job's container).
    let mut last_seen = tokio::time::Instant::now();
    loop {
        let mut params = LogParams {
            follow: options.follow,
            container: Some(container.clone()),
            timestamps: options.timestamps,
            previous: options.previous,
            ..Default::default()
        };
        if first_attempt {
            params.tail_lines = options.tail_lines;
            params.since_seconds = options.since_seconds;
        } else {
            let elapsed = last_seen.elapsed().as_secs().max(1);
            params.since_seconds = Some(elapsed as i64);
        }
        let mut received_data = false;
        match api.log_stream(&pod, &params).await {
            Ok(stream) => {
                first_attempt = false;
                let mut lines = stream.lines();
                while let Some(line) = lines.next().await {
                    match line {
                        Ok(text) => {
                            received_data = true;
                            last_seen = tokio::time::Instant::now();
                            if tx
                                .unbounded_send(StreamEvent::Line {
                                    pod: pod.clone(),
                                    container: container.clone(),
                                    text,
                                })
                                .is_err()
                            {
                                return;
                            }
                        }
                        Err(err) => {
                            tx.unbounded_send(StreamEvent::Error(err.to_string())).ok();
                            break;
                        }
                    }
                }
                if !options.follow {
                    return;
                }
            }
            Err(err) => {
                tx.unbounded_send(StreamEvent::Error(err.to_string())).ok();
            }
        }
        if tx
            .unbounded_send(StreamEvent::Reconnecting {
                pod: pod.clone(),
                container: container.clone(),
            })
            .is_err()
        {
            return;
        }
        // Only reset backoff once we've actually received data on a connection — a connection
        // that opens successfully but immediately errors/ends without delivering anything (e.g.
        // a terminated container being re-opened) must keep backing off, not spin at
        // `BACKOFF_START` forever.
        if received_data {
            backoff = BACKOFF_START;
        } else {
            backoff = (backoff * 2).min(BACKOFF_MAX);
        }
        tokio::time::sleep(backoff).await;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn container_names_include_init_only_when_asked() {
        let pod: Pod = serde_json::from_value(serde_json::json!({
            "spec": {
                "containers": [{"name": "app", "image": "x"}],
                "initContainers": [{"name": "migrate", "image": "x"}]
            }
        }))
        .unwrap();
        assert_eq!(container_names(&pod, false), vec!["app".to_string()]);
        assert_eq!(
            container_names(&pod, true),
            vec!["app".to_string(), "migrate".to_string()]
        );
    }
}
