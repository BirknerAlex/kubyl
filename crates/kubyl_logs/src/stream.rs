//! Kube log streaming for one pod or every pod of a label selector, off the UI thread. The view
//! drives it through [`run`] via `kubyl_core::spawn_kube` and consumes [`StreamEvent`]s.
//!
//! - Every container streams on its own task with reconnect-and-backoff. Timestamps are always
//!   requested: they're split off the text ([`split_timestamp`]) so level detection and JSON
//!   parsing see the raw message, the view can show or hide them, and a reconnect resumes at
//!   the last line's `sinceTime` without duplicating lines.
//! - The initial backlog of all containers is fetched first and merged by timestamp, so a
//!   3-replica deployment shows its history interleaved in order; live lines follow afterwards.
//! - Selector sources watch their pods (`kube::runtime::watcher`): new pods join as soon as
//!   their containers start, deleted pods leave.
//! - A container whose pod finished (or was deleted) ends its stream instead of reconnecting.

use std::collections::{HashMap, HashSet};
use std::time::Duration;

use futures::channel::mpsc;
use futures::{AsyncBufReadExt as _, StreamExt as _};
use jiff::Timestamp;
use k8s_openapi::api::core::v1::Pod;
use kube::api::{Api, LogParams};
use kube::runtime::{WatchStreamExt as _, watcher};
use tokio::sync::{oneshot, watch};

/// Where the lines of one log view come from.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum LogSource {
    /// One pod.
    Pod { pod: String },
    /// Every pod matching a label selector: a workload's `spec.selector`, a Service's selector
    /// or one the user typed. Pods join and leave live.
    Selector { label_selector: String },
}

/// Which containers of each pod stream.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct ContainerFilter {
    /// Only this container; `None` streams every regular container.
    pub only: Option<String>,
    /// Also stream init containers.
    pub init: bool,
    /// Also stream ephemeral (debug) containers.
    pub ephemeral: bool,
}

/// How far back the stream starts.
#[derive(Clone, Debug, Default, PartialEq)]
pub enum Since {
    /// From the start of the container's log (limited by `tail_lines`).
    #[default]
    Start,
    /// The last `n` seconds (`kubectl logs --since`).
    Seconds(i64),
    /// From a point in time (`kubectl logs --since-time`).
    Time(Timestamp),
}

#[derive(Clone, Debug, PartialEq)]
pub struct LogOptions {
    /// Keep streaming new lines (`kubectl logs -f`).
    pub follow: bool,
    pub since: Since,
    /// Lines requested per container when the stream starts (`--tail`); `None` = all.
    pub tail_lines: Option<i64>,
    /// The previous (crashed) instance of each container (`--previous`). Never follows.
    pub previous: bool,
}

impl Default for LogOptions {
    fn default() -> Self {
        Self {
            follow: true,
            since: Since::Start,
            tail_lines: Some(1000),
            previous: false,
        }
    }
}

impl LogOptions {
    fn follows(&self) -> bool {
        self.follow && !self.previous
    }
}

/// One log line as the API returned it, with the timestamp split off.
#[derive(Clone, Debug, PartialEq)]
pub struct RawLine {
    pub pod: String,
    pub container: String,
    pub timestamp: Option<Timestamp>,
    pub text: String,
}

#[derive(Clone, Debug, PartialEq)]
pub enum StreamEvent {
    /// Lines in order (the initial backlog merged by timestamp, then live lines).
    Lines(Vec<RawLine>),
    /// A pod started streaming. `initial`: it was there when the view opened.
    PodJoined {
        pod: String,
        containers: Vec<String>,
        initial: bool,
    },
    /// A pod was deleted (or left the selector).
    PodGone(String),
    /// A container's (re)connection is streaming.
    Connected {
        pod: String,
        container: String,
    },
    /// A container's stream broke; it reconnects after a backoff.
    Reconnecting {
        pod: String,
        container: String,
        error: Option<String>,
    },
    /// A container's log ended for good (its pod finished).
    Ended {
        pod: String,
        container: String,
        reason: String,
    },
    /// Every stream finished (not following).
    Done,
    Error(String),
}

const BACKOFF_START: Duration = Duration::from_secs(1);
const BACKOFF_MAX: Duration = Duration::from_secs(30);
/// How long the initial merge waits for slow containers before emitting what it has.
const BACKLOG_TIMEOUT: Duration = Duration::from_secs(15);
/// Lines per [`StreamEvent::Lines`] batch when emitting a merged backlog.
const BATCH: usize = 5000;

/// Splits the RFC3339 timestamp the API prefixes each line with (`timestamps=true`) off the
/// text. Lines without one (or from a server that ignored the flag) keep their full text.
pub fn split_timestamp(line: &str) -> (Option<Timestamp>, &str) {
    if let Some((prefix, rest)) = line.split_once(' ')
        && prefix.len() >= 20
        && prefix.as_bytes()[4] == b'-'
        && let Ok(timestamp) = prefix.parse::<Timestamp>()
    {
        return (Some(timestamp), rest);
    }
    (None, line)
}

/// Container names of a pod, grouped by kind, in spec order.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct PodContainers {
    pub regular: Vec<String>,
    pub init: Vec<String>,
    pub ephemeral: Vec<String>,
}

impl PodContainers {
    pub fn of(pod: &Pod) -> Self {
        let Some(spec) = &pod.spec else {
            return Self::default();
        };
        Self {
            regular: spec.containers.iter().map(|c| c.name.clone()).collect(),
            init: spec
                .init_containers
                .iter()
                .flatten()
                .map(|c| c.name.clone())
                .collect(),
            ephemeral: spec
                .ephemeral_containers
                .iter()
                .flatten()
                .map(|c| c.name.clone())
                .collect(),
        }
    }

    /// The containers `filter` selects.
    pub fn select(&self, filter: &ContainerFilter) -> Vec<String> {
        let mut names: Vec<String> = Vec::new();
        if filter.init {
            names.extend(self.init.iter().cloned());
        }
        names.extend(self.regular.iter().cloned());
        if filter.ephemeral {
            names.extend(self.ephemeral.iter().cloned());
        }
        match &filter.only {
            Some(only) => {
                let all = self.all();
                if all.iter().any(|n| n == only) {
                    vec![only.clone()]
                } else {
                    Vec::new()
                }
            }
            None => names,
        }
    }

    pub fn all(&self) -> Vec<String> {
        self.init
            .iter()
            .chain(&self.regular)
            .chain(&self.ephemeral)
            .cloned()
            .collect()
    }
}

/// Whether `container` of `pod` has started (running now or ran before), so a log request
/// won't be rejected with "container is waiting to start".
fn has_started(pod: &Pod, container: &str) -> bool {
    let Some(status) = &pod.status else {
        return false;
    };
    status
        .container_statuses
        .iter()
        .flatten()
        .chain(status.init_container_statuses.iter().flatten())
        .chain(status.ephemeral_container_statuses.iter().flatten())
        .find(|s| s.name == container)
        .is_some_and(|s| {
            let started = |state: Option<&k8s_openapi::api::core::v1::ContainerState>| {
                state.is_some_and(|st| st.running.is_some() || st.terminated.is_some())
            };
            started(s.state.as_ref()) || started(s.last_state.as_ref())
        })
}

/// Why a container's log won't produce more lines, if it won't: the pod finished, or the
/// container terminated and won't restart.
fn finished_reason(pod: &Pod, container: &str) -> Option<String> {
    let status = pod.status.as_ref()?;
    let phase = status.phase.as_deref().unwrap_or_default();
    let terminated = status
        .container_statuses
        .iter()
        .flatten()
        .chain(status.init_container_statuses.iter().flatten())
        .chain(status.ephemeral_container_statuses.iter().flatten())
        .find(|s| s.name == container)
        .and_then(|s| s.state.as_ref()?.terminated.as_ref());
    let restart_never =
        pod.spec.as_ref().and_then(|s| s.restart_policy.as_deref()) == Some("Never");
    let is_init = pod
        .spec
        .as_ref()
        .and_then(|s| s.init_containers.as_ref())
        .is_some_and(|c| c.iter().any(|c| c.name == container));
    let is_ephemeral = pod
        .spec
        .as_ref()
        .and_then(|s| s.ephemeral_containers.as_ref())
        .is_some_and(|c| c.iter().any(|c| c.name == container));
    match terminated {
        Some(t) if phase == "Succeeded" || phase == "Failed" || restart_never || is_ephemeral => {
            Some(format!("exited with code {}", t.exit_code))
        }
        // Init containers that completed successfully never run again.
        Some(t) if is_init && t.exit_code == 0 => Some("init container completed".into()),
        _ if phase == "Succeeded" || phase == "Failed" => Some(format!("pod {phase}")),
        _ => None,
    }
}

/// Streams `source` until the returned future is dropped (or the receiver of `tx` is gone).
/// Returns after [`StreamEvent::Done`] when not following.
pub async fn run(
    client: kube::Client,
    namespace: String,
    source: LogSource,
    containers: ContainerFilter,
    options: LogOptions,
    tx: mpsc::UnboundedSender<StreamEvent>,
) {
    let api: Api<Pod> = Api::namespaced(client, &namespace);
    match source {
        LogSource::Pod { pod } => {
            let object = match api.get(&pod).await {
                Ok(object) => object,
                Err(err) => {
                    tx.unbounded_send(StreamEvent::Error(err.to_string())).ok();
                    return;
                }
            };
            let names = PodContainers::of(&object).select(&containers);
            if names.is_empty() {
                let message = match &containers.only {
                    Some(only) => format!("pod {pod} has no container {only}"),
                    None => format!("pod {pod} has no containers"),
                };
                tx.unbounded_send(StreamEvent::Error(message)).ok();
                return;
            }
            tx.unbounded_send(StreamEvent::PodJoined {
                pod: pod.clone(),
                containers: names.clone(),
                initial: true,
            })
            .ok();
            let mut merge = Merge::new(tx.clone());
            let mut guards = Vec::new();
            for container in names {
                let backlog = merge.add();
                guards.push(spawn_container(
                    api.clone(),
                    pod.clone(),
                    container,
                    options.clone(),
                    Some(backlog),
                    merge.flushed(),
                    tx.clone(),
                ));
            }
            merge.run().await;
            if !options.follows() {
                // Every container returned right after delivering its backlog.
                tx.unbounded_send(StreamEvent::Done).ok();
                return;
            }
            std::future::pending::<()>().await;
            drop(guards);
        }
        LogSource::Selector { label_selector } => {
            selector_loop(api, label_selector, containers, options, tx).await;
        }
    }
}

/// Watches the pods of `label_selector`, starting a stream for every selected container once
/// it has started and stopping them when the pod goes away.
async fn selector_loop(
    api: Api<Pod>,
    label_selector: String,
    filter: ContainerFilter,
    options: LogOptions,
    tx: mpsc::UnboundedSender<StreamEvent>,
) {
    let config = watcher::Config::default().labels(&label_selector);
    let mut events = std::pin::pin!(watcher(api.clone(), config).default_backoff());
    let mut pods = PodStreams {
        api,
        filter,
        options,
        tx: tx.clone(),
        streams: HashMap::new(),
    };
    // The initial listing merges its backlogs; pods that join later stream directly.
    let mut initial = Some(Merge::new(tx.clone()));
    let mut listing: HashSet<String> = HashSet::new();

    while let Some(event) = events.next().await {
        match event {
            Ok(watcher::Event::Init) => listing.clear(),
            Ok(watcher::Event::InitApply(pod)) => {
                if let Some(name) = &pod.metadata.name {
                    listing.insert(name.clone());
                }
                pods.apply(&pod, initial.as_mut());
            }
            Ok(watcher::Event::InitDone) => {
                // Pods deleted while the watch was reconnecting.
                let gone: Vec<String> = pods
                    .streams
                    .keys()
                    .filter(|name| !listing.contains(*name))
                    .cloned()
                    .collect();
                for name in gone {
                    pods.remove(&name);
                }
                if let Some(merge) = initial.take() {
                    merge.run().await;
                    if !pods.options.follows() {
                        tx.unbounded_send(StreamEvent::Done).ok();
                        return;
                    }
                }
            }
            Ok(watcher::Event::Apply(pod)) => pods.apply(&pod, None),
            Ok(watcher::Event::Delete(pod)) => {
                if let Some(name) = &pod.metadata.name {
                    pods.remove(name);
                }
            }
            Err(err) => {
                tx.unbounded_send(StreamEvent::Error(err.to_string())).ok();
            }
        }
        if tx.is_closed() {
            return;
        }
    }
}

/// The container streams of a selector source, by pod and container. Dropping a guard aborts
/// its stream.
struct PodStreams {
    api: Api<Pod>,
    filter: ContainerFilter,
    options: LogOptions,
    tx: mpsc::UnboundedSender<StreamEvent>,
    streams: HashMap<String, HashMap<String, TaskGuard>>,
}

impl PodStreams {
    /// Starts streams for the pod's selected containers that have started and aren't streaming
    /// yet. `merge`: part of the initial listing, whose backlogs are merged.
    fn apply(&mut self, pod: &Pod, mut merge: Option<&mut Merge>) {
        let Some(name) = pod.metadata.name.clone() else {
            return;
        };
        if pod.metadata.deletion_timestamp.is_some() && !self.streams.contains_key(&name) {
            return;
        }
        let wanted = PodContainers::of(pod).select(&self.filter);
        let joined = !self.streams.contains_key(&name);
        let running = self.streams.entry(name.clone()).or_default();
        for container in &wanted {
            if running.contains_key(container) || !has_started(pod, container) {
                continue;
            }
            let backlog = merge.as_deref_mut().map(Merge::add);
            let flushed = merge.as_deref().map(Merge::flushed).unwrap_or_else(ready);
            running.insert(
                container.clone(),
                spawn_container(
                    self.api.clone(),
                    name.clone(),
                    container.clone(),
                    self.options.clone(),
                    backlog,
                    flushed,
                    self.tx.clone(),
                ),
            );
        }
        if joined {
            self.tx
                .unbounded_send(StreamEvent::PodJoined {
                    pod: name,
                    containers: wanted,
                    initial: merge.is_some(),
                })
                .ok();
        }
    }

    fn remove(&mut self, name: &str) {
        if self.streams.remove(name).is_some() {
            self.tx
                .unbounded_send(StreamEvent::PodGone(name.to_string()))
                .ok();
        }
    }
}

/// A receiver that is already "flushed" (no initial merge to wait for).
fn ready() -> watch::Receiver<bool> {
    watch::channel(true).1
}

/// Collects the initial backlog of several containers and emits it merged by timestamp.
struct Merge {
    tx: mpsc::UnboundedSender<StreamEvent>,
    backlogs: Vec<oneshot::Receiver<Vec<RawLine>>>,
    flushed_tx: watch::Sender<bool>,
    flushed_rx: watch::Receiver<bool>,
}

impl Merge {
    fn new(tx: mpsc::UnboundedSender<StreamEvent>) -> Self {
        let (flushed_tx, flushed_rx) = watch::channel(false);
        Self {
            tx,
            backlogs: Vec::new(),
            flushed_tx,
            flushed_rx,
        }
    }

    fn add(&mut self) -> oneshot::Sender<Vec<RawLine>> {
        let (tx, rx) = oneshot::channel();
        self.backlogs.push(rx);
        tx
    }

    /// Resolves once the merged backlog was emitted; containers start following after that.
    fn flushed(&self) -> watch::Receiver<bool> {
        self.flushed_rx.clone()
    }

    /// Waits for every backlog (or [`BACKLOG_TIMEOUT`]), emits them merged and lets the
    /// containers follow.
    async fn run(self) {
        let deadline = tokio::time::sleep(BACKLOG_TIMEOUT);
        let collect = futures::future::join_all(self.backlogs);
        let backlogs: Vec<Vec<RawLine>> = tokio::select! {
            results = collect => results.into_iter().filter_map(Result::ok).collect(),
            _ = deadline => Vec::new(),
        };
        let merged = merge_by_time(backlogs);
        for chunk in merged.chunks(BATCH) {
            if self
                .tx
                .unbounded_send(StreamEvent::Lines(chunk.to_vec()))
                .is_err()
            {
                break;
            }
        }
        self.flushed_tx.send(true).ok();
    }
}

/// Merges per-container line lists (each already in order) by timestamp. Lines without a
/// timestamp keep their position relative to their neighbors in the same list.
pub fn merge_by_time(lists: Vec<Vec<RawLine>>) -> Vec<RawLine> {
    let total = lists.iter().map(Vec::len).sum();
    let mut out = Vec::with_capacity(total);
    let mut iters: Vec<std::iter::Peekable<std::vec::IntoIter<RawLine>>> = lists
        .into_iter()
        .map(|l| l.into_iter().peekable())
        .collect();
    loop {
        let mut best: Option<(usize, Option<Timestamp>)> = None;
        for (ix, iter) in iters.iter_mut().enumerate() {
            let Some(line) = iter.peek() else { continue };
            let better = match (&best, line.timestamp) {
                (None, _) => true,
                // Untimed lines go out as soon as they are at the head of their list.
                (_, None) => true,
                (Some((_, None)), _) => false,
                (Some((_, Some(best_ts))), Some(ts)) => ts < *best_ts,
            };
            if better {
                best = Some((ix, line.timestamp));
                if line.timestamp.is_none() {
                    break;
                }
            }
        }
        let Some((ix, _)) = best else { break };
        if let Some(line) = iters[ix].next() {
            out.push(line);
        }
    }
    out
}

/// Aborts its task when dropped.
struct TaskGuard(tokio::task::JoinHandle<()>);

impl Drop for TaskGuard {
    fn drop(&mut self) {
        self.0.abort();
    }
}

fn spawn_container(
    api: Api<Pod>,
    pod: String,
    container: String,
    options: LogOptions,
    backlog: Option<oneshot::Sender<Vec<RawLine>>>,
    flushed: watch::Receiver<bool>,
    tx: mpsc::UnboundedSender<StreamEvent>,
) -> TaskGuard {
    TaskGuard(tokio::spawn(container_task(
        api, pod, container, options, backlog, flushed, tx,
    )))
}

/// Tracks the last delivered timestamp so a reconnect (which can only ask for whole seconds)
/// skips lines already shown.
#[derive(Default)]
struct Resume {
    last: Option<Timestamp>,
    /// Lines delivered with exactly `last`.
    at_last: usize,
}

impl Resume {
    fn record(&mut self, timestamp: Option<Timestamp>) {
        let Some(ts) = timestamp else { return };
        if self.last == Some(ts) {
            self.at_last += 1;
        } else if self.last.is_none_or(|last| ts > last) {
            self.last = Some(ts);
            self.at_last = 1;
        }
    }

    /// Whether a line from a resumed stream was already delivered.
    fn is_duplicate(&self, timestamp: Option<Timestamp>, seen_at_last: &mut usize) -> bool {
        let (Some(last), Some(ts)) = (self.last, timestamp) else {
            return false;
        };
        if ts < last {
            return true;
        }
        if ts == last && *seen_at_last < self.at_last {
            *seen_at_last += 1;
            return true;
        }
        false
    }
}

fn parse_line(pod: &str, container: &str, bytes: &[u8]) -> RawLine {
    let text = String::from_utf8_lossy(bytes);
    let text = text.trim_end_matches(['\n', '\r']);
    let (timestamp, text) = split_timestamp(text);
    RawLine {
        pod: pod.to_string(),
        container: container.to_string(),
        timestamp,
        text: text.to_string(),
    }
}

fn params(container: &str, options: &LogOptions, follow: bool) -> LogParams {
    LogParams {
        follow,
        container: Some(container.to_string()),
        timestamps: true,
        previous: options.previous,
        tail_lines: options.tail_lines,
        since_seconds: match options.since {
            Since::Seconds(s) => Some(s),
            _ => None,
        },
        since_time: match options.since {
            Since::Time(t) => Some(t),
            _ => None,
        },
        ..Default::default()
    }
}

/// Fetches a finished (non-follow) log in one request.
async fn fetch(
    api: &Api<Pod>,
    pod: &str,
    container: &str,
    params: &LogParams,
) -> kube::Result<Vec<RawLine>> {
    let text = api.logs(pod, params).await?;
    Ok(text
        .lines()
        .map(|line| parse_line(pod, container, line.as_bytes()))
        .collect())
}

/// One container: the initial backlog (merged by the caller when `backlog` is set), then the
/// live stream with reconnects.
async fn container_task(
    api: Api<Pod>,
    pod: String,
    container: String,
    options: LogOptions,
    backlog: Option<oneshot::Sender<Vec<RawLine>>>,
    mut flushed: watch::Receiver<bool>,
    tx: mpsc::UnboundedSender<StreamEvent>,
) {
    let mut resume = Resume::default();
    let mut first = true;
    if let Some(backlog) = backlog {
        let lines = match fetch(&api, &pod, &container, &params(&container, &options, false)).await
        {
            Ok(lines) => lines,
            Err(err) => {
                tx.unbounded_send(StreamEvent::Reconnecting {
                    pod: pod.clone(),
                    container: container.clone(),
                    error: Some(err.to_string()),
                })
                .ok();
                Vec::new()
            }
        };
        for line in &lines {
            resume.record(line.timestamp);
        }
        first = false;
        backlog.send(lines).ok();
        if !options.follows() {
            return;
        }
        // Live lines must come after the merged backlog.
        if flushed.wait_for(|done| *done).await.is_err() {
            return;
        }
    } else if !options.follows() {
        match fetch(&api, &pod, &container, &params(&container, &options, false)).await {
            Ok(lines) => {
                tx.unbounded_send(StreamEvent::Lines(lines)).ok();
            }
            Err(err) => {
                tx.unbounded_send(StreamEvent::Error(err.to_string())).ok();
            }
        }
        return;
    }

    let mut backoff = BACKOFF_START;
    loop {
        let mut request = params(&container, &options, true);
        if !first {
            request.tail_lines = None;
            request.since_seconds = None;
            request.since_time = resume.last;
            if resume.last.is_none() {
                // Nothing delivered yet: don't replay a long history on every retry.
                request.tail_lines = options.tail_lines;
            }
        }
        first = false;
        let mut received = false;
        let mut error = None;
        match api.log_stream(&pod, &request).await {
            Ok(stream) => {
                tx.unbounded_send(StreamEvent::Connected {
                    pod: pod.clone(),
                    container: container.clone(),
                })
                .ok();
                let mut stream = std::pin::pin!(stream);
                let mut buf = Vec::new();
                let mut seen_at_last = 0;
                loop {
                    buf.clear();
                    match stream.read_until(b'\n', &mut buf).await {
                        Ok(0) => break,
                        Ok(_) => {
                            let line = parse_line(&pod, &container, &buf);
                            if resume.is_duplicate(line.timestamp, &mut seen_at_last) {
                                continue;
                            }
                            received = true;
                            resume.record(line.timestamp);
                            if tx.unbounded_send(StreamEvent::Lines(vec![line])).is_err() {
                                return;
                            }
                        }
                        Err(err) => {
                            error = Some(err.to_string());
                            break;
                        }
                    }
                }
            }
            Err(err) => error = Some(err.to_string()),
        }

        // Did the stream end because the pod finished or went away?
        match api.get_opt(&pod).await {
            Ok(None) => {
                tx.unbounded_send(StreamEvent::PodGone(pod.clone())).ok();
                return;
            }
            Ok(Some(object)) => {
                if let Some(reason) = finished_reason(&object, &container) {
                    tx.unbounded_send(StreamEvent::Ended {
                        pod: pod.clone(),
                        container: container.clone(),
                        reason,
                    })
                    .ok();
                    return;
                }
            }
            Err(_) => {}
        }
        if tx
            .unbounded_send(StreamEvent::Reconnecting {
                pod: pod.clone(),
                container: container.clone(),
                error,
            })
            .is_err()
        {
            return;
        }
        // Only reset the backoff after a connection delivered data: one that opens and ends
        // right away (a restarting container) keeps backing off.
        backoff = if received {
            BACKOFF_START
        } else {
            (backoff * 2).min(BACKOFF_MAX)
        };
        tokio::time::sleep(backoff).await;
    }
}

/// Fetches the complete log of `targets` (`(pod, container)` pairs) without following,
/// merged by timestamp, for "Download full log".
pub async fn fetch_all(
    client: kube::Client,
    namespace: String,
    targets: Vec<(String, String)>,
    previous: bool,
) -> anyhow::Result<Vec<RawLine>> {
    let api: Api<Pod> = Api::namespaced(client, &namespace);
    let options = LogOptions {
        follow: false,
        since: Since::Start,
        tail_lines: None,
        previous,
    };
    let requests = targets.iter().map(|(pod, container)| {
        let api = api.clone();
        let request = params(container, &options, false);
        async move { fetch(&api, pod, container, &request).await }
    });
    let mut lists = Vec::new();
    for result in futures::future::join_all(requests).await {
        lists.push(result?);
    }
    Ok(merge_by_time(lists))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn line(pod: &str, ts: Option<&str>, text: &str) -> RawLine {
        RawLine {
            pod: pod.into(),
            container: "c".into(),
            timestamp: ts.map(|t| t.parse().unwrap()),
            text: text.into(),
        }
    }

    #[test]
    fn splits_the_api_timestamp() {
        let (ts, text) = split_timestamp("2026-09-25T10:42:17.902123456Z {\"level\":\"info\"}");
        assert_eq!(
            ts.unwrap().to_string(),
            "2026-09-25T10:42:17.902123456Z".to_string()
        );
        assert_eq!(text, "{\"level\":\"info\"}");
        let (ts, text) = split_timestamp("ERROR: no timestamp here");
        assert!(ts.is_none());
        assert_eq!(text, "ERROR: no timestamp here");
    }

    #[test]
    fn merges_backlogs_by_time() {
        let a = vec![
            line("a", Some("2026-01-01T00:00:01Z"), "a1"),
            line("a", Some("2026-01-01T00:00:03Z"), "a3"),
        ];
        let b = vec![
            line("b", Some("2026-01-01T00:00:02Z"), "b2"),
            line("b", Some("2026-01-01T00:00:04Z"), "b4"),
        ];
        let c = vec![line("c", Some("2026-01-01T00:00:00Z"), "c0")];
        let merged: Vec<String> = merge_by_time(vec![a, b, c])
            .into_iter()
            .map(|l| l.text)
            .collect();
        assert_eq!(merged, ["c0", "a1", "b2", "a3", "b4"]);
    }

    #[test]
    fn untimed_lines_keep_their_place() {
        let a = vec![
            line("a", Some("2026-01-01T00:00:05Z"), "a5"),
            line("a", None, "a-cont"),
        ];
        let b = vec![line("b", Some("2026-01-01T00:00:06Z"), "b6")];
        let merged: Vec<String> = merge_by_time(vec![a, b])
            .into_iter()
            .map(|l| l.text)
            .collect();
        assert_eq!(merged, ["a5", "a-cont", "b6"]);
    }

    #[test]
    fn resume_skips_lines_already_delivered() {
        let mut resume = Resume::default();
        let t1: Timestamp = "2026-01-01T00:00:01.5Z".parse().unwrap();
        let t2: Timestamp = "2026-01-01T00:00:02Z".parse().unwrap();
        resume.record(Some(t1));
        resume.record(Some(t2));
        resume.record(Some(t2));
        let mut seen = 0;
        // The reconnect replays the whole second: t1 and both t2 lines are old.
        assert!(resume.is_duplicate(Some(t1), &mut seen));
        assert!(resume.is_duplicate(Some(t2), &mut seen));
        assert!(resume.is_duplicate(Some(t2), &mut seen));
        // A third line at t2 is new.
        assert!(!resume.is_duplicate(Some(t2), &mut seen));
        assert!(!resume.is_duplicate(None, &mut seen));
    }

    fn pod(value: serde_json::Value) -> Pod {
        serde_json::from_value(value).unwrap()
    }

    #[test]
    fn selects_containers() {
        let p = pod(serde_json::json!({
            "spec": {
                "containers": [{"name": "app"}, {"name": "sidecar"}],
                "initContainers": [{"name": "migrate"}],
                "ephemeralContainers": [{"name": "debugger"}]
            }
        }));
        let containers = PodContainers::of(&p);
        assert_eq!(
            containers.select(&ContainerFilter::default()),
            ["app", "sidecar"]
        );
        assert_eq!(
            containers.select(&ContainerFilter {
                init: true,
                ephemeral: true,
                ..Default::default()
            }),
            ["migrate", "app", "sidecar", "debugger"]
        );
        assert_eq!(
            containers.select(&ContainerFilter {
                only: Some("debugger".into()),
                ..Default::default()
            }),
            ["debugger"]
        );
        assert!(
            containers
                .select(&ContainerFilter {
                    only: Some("nope".into()),
                    ..Default::default()
                })
                .is_empty()
        );
    }

    #[test]
    fn started_and_finished_containers() {
        let running = pod(serde_json::json!({
            "spec": {"containers": [{"name": "app"}]},
            "status": {"phase": "Running", "containerStatuses": [
                {"name": "app", "ready": true, "restartCount": 0, "image": "x", "imageID": "",
                 "state": {"running": {}}}
            ]}
        }));
        assert!(has_started(&running, "app"));
        assert_eq!(finished_reason(&running, "app"), None);

        let waiting = pod(serde_json::json!({
            "spec": {"containers": [{"name": "app"}]},
            "status": {"phase": "Pending", "containerStatuses": [
                {"name": "app", "ready": false, "restartCount": 0, "image": "x", "imageID": "",
                 "state": {"waiting": {"reason": "ContainerCreating"}}}
            ]}
        }));
        assert!(!has_started(&waiting, "app"));

        let done = pod(serde_json::json!({
            "spec": {"containers": [{"name": "app"}], "restartPolicy": "Never"},
            "status": {"phase": "Succeeded", "containerStatuses": [
                {"name": "app", "ready": false, "restartCount": 0, "image": "x", "imageID": "",
                 "state": {"terminated": {"exitCode": 0}}}
            ]}
        }));
        assert!(has_started(&done, "app"));
        assert_eq!(
            finished_reason(&done, "app").as_deref(),
            Some("exited with code 0")
        );

        // A crashlooping container (restartPolicy Always) isn't finished.
        let crashing = pod(serde_json::json!({
            "spec": {"containers": [{"name": "app"}]},
            "status": {"phase": "Running", "containerStatuses": [
                {"name": "app", "ready": false, "restartCount": 3, "image": "x", "imageID": "",
                 "state": {"terminated": {"exitCode": 1}}}
            ]}
        }));
        assert_eq!(finished_reason(&crashing, "app"), None);
    }
}
