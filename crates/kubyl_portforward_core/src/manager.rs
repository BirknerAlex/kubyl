//! [`ForwardsCore`]: the running port-forwards, their state (listening, reconnecting, failed),
//! connections and bytes, as a plain state machine on a [`Host`].
//!
//! A forward whose target stops resolving (the pod died, the Service has no ready endpoints) is
//! "reconnecting": the local port stays open and a probe re-resolves the target every few
//! seconds until a pod answers again. New connections always resolve afresh, so a Service
//! forward survives pod restarts.
//!
//! `kubyl_portforward::manager::PortForwardManager` holds one in an entity and shows each
//! forward as a row in the Active Sessions panel.

use std::collections::HashMap;
use std::time::Duration;

use futures::channel::{mpsc, oneshot};
use kubyl_base::host::{Flow, Host, HostExt as _, Pace, Service, TaskHandle};
use kubyl_base::types::ActiveForward;
use kubyl_base::{ClusterId, Gvr, Notice, ResourceRef, Tone};

use crate::listener::{self, ForwardEvent};
use crate::reach::{Opened, ReachCommand, ReachInbox, ReachPort, ReachRequest};
use crate::resolve::{self, ForwardKind, RemotePort};

/// How often a reconnecting forward re-resolves its target.
const PROBE_INTERVAL: Duration = Duration::from_secs(4);
/// Listener events are applied at most this often (a connection burst arrives together).
const EVENT_INTERVAL: Duration = Duration::from_millis(100);

/// What to forward, without the parts only the UI knows about.
#[derive(Clone, Debug)]
pub struct Target {
    pub cluster: ClusterId,
    pub namespace: String,
    pub kind: ForwardKind,
    pub port: RemotePort,
    pub bind_address: String,
    /// `0` = pick a free port.
    pub local_port: u16,
    /// What was forwarded (Pod, Service or workload), for labels and saving.
    pub target: ResourceRef,
    /// The port as the user picked it (`None` = the first one), for labels and saving.
    pub remote_port: Option<u16>,
    /// Offer "Open in browser" (and `https` when set).
    pub http: bool,
    pub https: bool,
    /// Open the browser once the port listens.
    pub open_browser: bool,
    /// The session title of a temporary forward owned by another feature (web views). Those
    /// are never saved and not listed in [`ForwardsCore::active`].
    pub ephemeral: Option<String>,
}

impl Target {
    /// `svc/ledger :5432`, or the title of a temporary forward.
    pub fn session_label(&self) -> String {
        match &self.ephemeral {
            Some(title) => title.clone(),
            None => self.target_label(),
        }
    }

    /// `svc/ledger :5432`.
    pub fn target_label(&self) -> String {
        let kind = short_kind(&self.target.gvr.resource);
        let name = self.target.name.clone().unwrap_or_default();
        match self.remote_port {
            Some(port) => format!("{kind}/{name} :{port}"),
            None => format!("{kind}/{name}"),
        }
    }
}

/// `svc`, `deploy`… for a resource plural.
pub fn short_kind(resource: &str) -> &str {
    match resource {
        "services" => "svc",
        "pods" => "pod",
        "deployments" => "deploy",
        "statefulsets" => "sts",
        "daemonsets" => "ds",
        other => other,
    }
}

#[derive(Clone, Debug, PartialEq)]
pub enum ForwardState {
    Starting,
    Listening,
    /// The target doesn't resolve (or connections fail); probing until it does.
    Reconnecting(String),
    /// Binding the local port failed.
    Failed(String),
}

/// A running forward's state, for crates that show their own forwards.
#[derive(Clone, Debug, PartialEq)]
pub struct ForwardInfo {
    pub state: ForwardState,
    /// `0` until the port listens.
    pub local_port: u16,
    /// The pod the last connection (or check) reached.
    pub pod: Option<String>,
    /// The remote port of the last resolution.
    pub remote_port: Option<u16>,
    pub connections: u64,
    pub bytes_sent: u64,
    pub bytes_received: u64,
}

/// One running forward.
pub struct Forward {
    target: Target,
    state: ForwardState,
    local_port: u16,
    /// The remote port of the last resolution.
    resolved_port: Option<u16>,
    /// The pod of the last resolution or connection.
    resolved_pod: Option<String>,
    connections: u64,
    bytes_sent: u64,
    bytes_received: u64,
    /// The client the forward was started with (probes use it too).
    client: kube::Client,
    /// The listener on Tokio and the task applying its events.
    _tasks: Vec<TaskHandle>,
    probe: Option<TaskHandle>,
}

impl Forward {
    pub fn target(&self) -> &Target {
        &self.target
    }

    pub fn state(&self) -> &ForwardState {
        &self.state
    }

    pub fn local_port(&self) -> u16 {
        self.local_port
    }

    /// `http://localhost:18080`.
    pub fn url(&self) -> String {
        let scheme = if self.target.https { "https" } else { "http" };
        format!("{scheme}://{}", self.local())
    }

    /// `host:port` to reach the forward from this machine (wildcard binds show `localhost`).
    pub fn local(&self) -> String {
        host_port(&display_host(&self.target.bind_address), self.local_port)
    }

    /// `bind:port`, as listening.
    pub fn address(&self) -> String {
        host_port(&self.target.bind_address, self.local_port)
    }

    /// `svc/ledger :5432 → :15432`.
    pub fn title(&self) -> String {
        let target = self.target.session_label();
        if self.local_port == 0 {
            target
        } else {
            format!("{target} → :{}", self.local_port)
        }
    }

    /// The status line of its Active Sessions row.
    pub fn status(&self) -> (String, Tone) {
        match &self.state {
            ForwardState::Starting => ("starting…".into(), Tone::Info),
            ForwardState::Failed(err) => (format!("error: {err}"), Tone::Bad),
            ForwardState::Reconnecting(err) => (format!("reconnecting… {err}"), Tone::Warning),
            ForwardState::Listening => {
                let conns = match self.connections {
                    1 => "1 conn".to_string(),
                    n => format!("{n} conns"),
                };
                let mut status = format!("port-forward · {} · {conns}", self.address());
                if self.bytes_sent + self.bytes_received > 0 {
                    status.push_str(&format!(
                        " · {} ↑ {} ↓",
                        human_bytes(self.bytes_sent),
                        human_bytes(self.bytes_received)
                    ));
                }
                (status, Tone::Good)
            }
        }
    }

    /// Listening or reconnecting: there's a local port to open or copy.
    pub fn is_live(&self) -> bool {
        matches!(
            self.state,
            ForwardState::Listening | ForwardState::Reconnecting(_)
        )
    }
}

/// The host a client on this machine uses for a bind address.
fn display_host(bind: &str) -> String {
    match bind {
        "0.0.0.0" | "::" | "" => "localhost".into(),
        other => other.into(),
    }
}

/// `host:port`, with an IPv6 literal in brackets.
fn host_port(host: &str, port: u16) -> String {
    if host.contains(':') && !host.starts_with('[') {
        format!("[{host}]:{port}")
    } else {
        format!("{host}:{port}")
    }
}

/// What the core asks its host to do.
pub enum ForwardsEffect {
    /// A forward's state, ports or counters changed: refresh its row.
    Changed(u64),
    /// Open `url` in the browser.
    OpenUrl(String),
    /// A forward started because a feature asked to reach a Service ([`ForwardsCore::serve`]):
    /// show it like one started by the user.
    Reaching(u64),
    /// The feature that asked for forward `id` let go of it; it is gone.
    Released(u64),
}

/// The running forwards.
#[derive(Default)]
pub struct ForwardsCore {
    forwards: HashMap<u64, Forward>,
    next_id: u64,
    /// Requests of [`ForwardsCore::serve`] waiting for their forward to listen.
    waiting: HashMap<u64, Waiting>,
    /// Told when a forward asked for through [`ForwardsCore::serve`] goes away.
    watchers: HashMap<u64, oneshot::Sender<()>>,
}

struct Waiting {
    reply: oneshot::Sender<Result<Opened, String>>,
    reconnect_grace: Option<Duration>,
    /// The timeout, and the grace period once the forward is reconnecting.
    _timers: Vec<TaskHandle>,
}

impl Service for ForwardsCore {
    /// The list of forwards changed (one started or stopped).
    type Event = ();
    type Effect = ForwardsEffect;
}

impl ForwardsCore {
    /// The id the next [`Self::start`] uses.
    pub fn next_id(&mut self) -> u64 {
        let id = self.next_id;
        self.next_id += 1;
        id
    }

    pub fn forward(&self, id: u64) -> Option<&Forward> {
        self.forwards.get(&id)
    }

    pub fn forwards(&self) -> impl Iterator<Item = (u64, &Forward)> {
        self.forwards.iter().map(|(id, f)| (*id, f))
    }

    /// Starts forward `id` (from [`Self::next_id`]); `fallback_any`: a taken local port falls
    /// back to a free one.
    pub fn start(
        &mut self,
        id: u64,
        client: kube::Client,
        target: Target,
        fallback_any: bool,
        host: &mut dyn Host<Self>,
    ) {
        let (events_tx, events_rx) = mpsc::unbounded();
        let (namespace, kind, port, bind_address, local_port) = (
            target.namespace.clone(),
            target.kind.clone(),
            target.port.clone(),
            target.bind_address.clone(),
            target.local_port,
        );
        let listen_client = client.clone();
        let listener = host.spawn(
            async move {
                listener::run(
                    listen_client,
                    namespace,
                    kind,
                    port,
                    bind_address,
                    local_port,
                    fallback_any,
                    events_tx,
                )
                .await
            },
            move |this, result, host| {
                // The listener returned: binding the local port failed.
                if let Err(err) = result {
                    this.set_state(id, ForwardState::Failed(format!("{err:#}")), host);
                    host.toast(Notice::error(format!("Port-forward failed: {err:#}")));
                }
            },
        );
        let events = host.batches(
            events_rx,
            Pace::throttle(EVENT_INTERVAL),
            move |this, events: Vec<ForwardEvent>, host| {
                for event in &events {
                    if !this.apply_event(id, event, host) {
                        return Flow::Stop;
                    }
                }
                Flow::Continue
            },
        );
        self.forwards.insert(
            id,
            Forward {
                target,
                state: ForwardState::Starting,
                local_port,
                resolved_port: None,
                resolved_pod: None,
                connections: 0,
                bytes_sent: 0,
                bytes_received: 0,
                client: client.clone(),
                _tasks: vec![listener, events],
                probe: None,
            },
        );
        self.check_target(id, client, host);
        host.effect(ForwardsEffect::Changed(id));
        host.emit(());
        host.notify();
    }

    /// Drops a forward. Returns it, so the host can clean up what it kept for it.
    pub fn remove(&mut self, id: u64, host: &mut dyn Host<Self>) -> Option<Target> {
        let forward = self.forwards.remove(&id)?;
        self.watchers.remove(&id);
        if let Some(waiting) = self.waiting.remove(&id) {
            waiting
                .reply
                .send(Err("the port-forward stopped".into()))
                .ok();
        }
        host.emit(());
        host.notify();
        Some(forward.target)
    }

    /// The clusters that have forwards.
    pub fn clusters(&self) -> Vec<ClusterId> {
        let mut clusters: Vec<ClusterId> = Vec::new();
        for f in self.forwards.values() {
            if !clusters.contains(&f.target.cluster) {
                clusters.push(f.target.cluster.clone());
            }
        }
        clusters
    }

    /// The forwards whose cluster isn't connected any more (disconnected, reconnecting with a
    /// new client, or removed from the kubeconfig): they hold the old client and would keep a
    /// dead listener open.
    pub fn disconnected(&self, connected: impl Fn(&ClusterId) -> bool) -> Vec<u64> {
        self.forwards
            .iter()
            .filter(|(_, f)| !connected(&f.target.cluster))
            .map(|(id, _)| *id)
            .collect()
    }

    /// A cluster entry's id changed without reconnecting: its forwards follow.
    pub fn rekey(&mut self, from: &ClusterId, to: &ClusterId) {
        for forward in self.forwards.values_mut() {
            if forward.target.cluster == *from {
                forward.target.cluster = to.clone();
            }
        }
    }

    /// A forward's state, or `None` once it stopped.
    pub fn info(&self, id: u64) -> Option<ForwardInfo> {
        let forward = self.forwards.get(&id)?;
        Some(ForwardInfo {
            state: forward.state.clone(),
            local_port: forward.local_port,
            pod: forward.resolved_pod.clone(),
            remote_port: forward.resolved_port,
            connections: forward.connections,
            bytes_sent: forward.bytes_sent,
            bytes_received: forward.bytes_received,
        })
    }

    /// The running forwards as other crates see them (temporary ones aren't listed).
    pub fn active(&self) -> Vec<ActiveForward> {
        let mut forwards: Vec<ActiveForward> = self
            .forwards
            .iter()
            .filter(|(_, forward)| forward.target.ephemeral.is_none())
            .map(|(id, forward)| {
                let listening = forward.state == ForwardState::Listening && forward.local_port != 0;
                ActiveForward {
                    id: *id,
                    target: forward.target.target.clone(),
                    remote_port: forward.target.remote_port,
                    local: listening.then(|| forward.local()),
                    url: (listening && forward.target.http).then(|| forward.url()),
                }
            })
            .collect();
        forwards.sort_by_key(|f| f.id);
        forwards
    }

    /// Resolves the target once (on Tokio), to show the remote port and to report a target that
    /// doesn't resolve before the first connection. Failing resolutions put the forward into
    /// "reconnecting" and keep probing.
    fn check_target(&mut self, id: u64, client: kube::Client, host: &mut dyn Host<Self>) {
        let Some(forward) = self.forwards.get(&id) else {
            return;
        };
        let (namespace, kind, port) = (
            forward.target.namespace.clone(),
            forward.target.kind.clone(),
            forward.target.port.clone(),
        );
        let probe = host.spawn(
            async move {
                resolve::resolve(client, &namespace, &kind, port)
                    .await
                    .map_err(|e| format!("{e:#}"))
            },
            move |this, result, host| this.finish_check(id, result, host),
        );
        if let Some(forward) = self.forwards.get_mut(&id) {
            forward.probe = Some(probe);
        }
    }

    fn finish_check(
        &mut self,
        id: u64,
        result: Result<resolve::Resolved, String>,
        host: &mut dyn Host<Self>,
    ) {
        let Some(forward) = self.forwards.get_mut(&id) else {
            return;
        };
        match result {
            Ok(resolved) => {
                forward.resolved_port = Some(resolved.port);
                forward.resolved_pod = Some(resolved.pod);
                if matches!(forward.state, ForwardState::Reconnecting(_)) {
                    forward.state = ForwardState::Listening;
                }
                // Done: a later error must be able to probe again. (Dropping the running probe
                // from inside is fine, nothing runs after it.)
                forward.probe = None;
                host.effect(ForwardsEffect::Changed(id));
                // A request may wait for this forward to listen again.
                self.settle(id, host);
            }
            // Binding the local port failed: nothing to reconnect.
            Err(_) if matches!(forward.state, ForwardState::Failed(_)) => {
                forward.probe = None;
            }
            Err(err) => {
                forward.state = ForwardState::Reconnecting(err);
                host.effect(ForwardsEffect::Changed(id));
                self.settle(id, host);
                let probe = host.after(PROBE_INTERVAL, move |this, host| {
                    if let Some(client) = this.forwards.get(&id).map(|f| f.client.clone()) {
                        this.check_target(id, client, host);
                    }
                });
                if let Some(forward) = self.forwards.get_mut(&id) {
                    forward.probe = Some(probe);
                }
            }
        }
    }

    fn set_state(&mut self, id: u64, state: ForwardState, host: &mut dyn Host<Self>) {
        if let Some(forward) = self.forwards.get_mut(&id) {
            forward.state = state;
            host.effect(ForwardsEffect::Changed(id));
            self.settle(id, host);
        }
    }

    /// Applies a listener event. Returns `false` once the forward is gone.
    fn apply_event(&mut self, id: u64, event: &ForwardEvent, host: &mut dyn Host<Self>) -> bool {
        let Some(forward) = self.forwards.get_mut(&id) else {
            return false;
        };
        match event {
            ForwardEvent::Listening { local_port } => {
                forward.local_port = *local_port;
                if forward.state == ForwardState::Starting {
                    forward.state = ForwardState::Listening;
                }
                if forward.target.open_browser && forward.target.http {
                    host.effect(ForwardsEffect::OpenUrl(forward.url()));
                }
            }
            ForwardEvent::ConnectionOpened { pod } => {
                forward.connections += 1;
                forward.resolved_pod = Some(pod.clone());
                // A connection reached a pod: the target resolves again.
                if matches!(forward.state, ForwardState::Reconnecting(_)) {
                    forward.state = ForwardState::Listening;
                    forward.probe = None;
                }
            }
            ForwardEvent::ConnectionClosed => {
                forward.connections = forward.connections.saturating_sub(1);
            }
            ForwardEvent::BytesTransferred { sent, received } => {
                forward.bytes_sent += sent;
                forward.bytes_received += received;
            }
            ForwardEvent::Error(message) => {
                tracing::debug!(forward = %forward.target.session_label(), "port-forward error: {message}");
                if !matches!(forward.state, ForwardState::Failed(_)) {
                    forward.state = ForwardState::Reconnecting(message.clone());
                    if forward.probe.is_none() {
                        let client = forward.client.clone();
                        self.check_target(id, client, host);
                    }
                }
            }
        }
        host.effect(ForwardsEffect::Changed(id));
        self.settle(id, host);
        true
    }

    /// Answers the request waiting for forward `id` once it listens or failed to bind, and starts
    /// the grace period of one that is reconnecting.
    fn settle(&mut self, id: u64, host: &mut dyn Host<Self>) {
        let Some(forward) = self.forwards.get(&id) else {
            return;
        };
        match &forward.state {
            ForwardState::Listening if forward.local_port != 0 => {
                let local_port = forward.local_port;
                if let Some(waiting) = self.waiting.remove(&id) {
                    let (stopped_tx, stopped) = oneshot::channel();
                    let opened = Opened {
                        id,
                        local_port,
                        stopped,
                    };
                    if waiting.reply.send(Ok(opened)).is_ok() {
                        self.watchers.insert(id, stopped_tx);
                    } else {
                        // Nobody waits any more.
                        self.release(id, host);
                    }
                }
            }
            ForwardState::Failed(err) => {
                let err = format!("port-forward: {err}");
                self.give_up(id, err, host);
            }
            ForwardState::Reconnecting(_) => {
                let Some(waiting) = self.waiting.get_mut(&id) else {
                    return;
                };
                if let Some(grace) = waiting.reconnect_grace.take() {
                    let timer = host.after(grace, move |this, host| {
                        if let Some(ForwardState::Reconnecting(err)) =
                            this.forwards.get(&id).map(|f| f.state.clone())
                        {
                            this.give_up(id, format!("port-forward: {err}"), host);
                        }
                    });
                    waiting._timers.push(timer);
                }
            }
            _ => {}
        }
    }

    /// Fails the request waiting for forward `id` and stops the forward.
    fn give_up(&mut self, id: u64, error: String, host: &mut dyn Host<Self>) {
        if let Some(waiting) = self.waiting.remove(&id) {
            waiting.reply.send(Err(error)).ok();
            self.release(id, host);
        }
    }

    /// Drops a forward a feature asked for and tells the host to forget its row.
    fn release(&mut self, id: u64, host: &mut dyn Host<Self>) {
        self.remove(id, host);
        host.effect(ForwardsEffect::Released(id));
    }

    /// Answers the requests of a [`crate::reach::ChannelReach`] on `host`: opens a loopback
    /// forward per request and stops it when the feature drops what it got. Keep the handle
    /// for as long as the service should answer.
    pub fn serve(&mut self, inbox: ReachInbox, host: &mut dyn Host<Self>) -> TaskHandle {
        host.batches(
            inbox,
            Pace::IMMEDIATE,
            |this, commands: Vec<ReachCommand>, host| {
                for command in commands {
                    this.handle(command, host);
                }
                Flow::Continue
            },
        )
    }

    /// Carries out one request of a [`crate::reach::ChannelReach`].
    pub fn handle(&mut self, command: ReachCommand, host: &mut dyn Host<Self>) {
        match command {
            ReachCommand::Stop(id) => {
                if self.forwards.contains_key(&id) {
                    self.release(id, host);
                }
            }
            ReachCommand::Open { request, reply } => {
                let ReachRequest {
                    client,
                    cluster,
                    namespace,
                    service,
                    port,
                    title,
                    https,
                    timeout,
                    reconnect_grace,
                } = *request;
                let ReachPort::Number(number) = port else {
                    reply.send(Err("the port isn't resolved".into())).ok();
                    return;
                };
                let target = Target {
                    cluster: cluster.clone(),
                    namespace: namespace.clone(),
                    kind: ForwardKind::Service {
                        service: service.clone(),
                    },
                    port: RemotePort::Service(Some(number)),
                    // Loopback only.
                    bind_address: "127.0.0.1".into(),
                    local_port: 0,
                    target: ResourceRef::object(
                        cluster,
                        Gvr::new("", "v1", "services"),
                        Some(namespace),
                        service,
                    ),
                    remote_port: Some(number),
                    http: false,
                    https,
                    open_browser: false,
                    ephemeral: Some(title),
                };
                let id = self.next_id();
                self.start(id, client, target, false, host);
                host.effect(ForwardsEffect::Reaching(id));
                let expire = host.after(timeout, move |this, host| {
                    this.give_up(
                        id,
                        "the port-forward didn't start (needs create pods/portforward)".into(),
                        host,
                    );
                });
                self.waiting.insert(
                    id,
                    Waiting {
                        reply,
                        reconnect_grace,
                        _timers: vec![expire],
                    },
                );
                self.settle(id, host);
            }
        }
    }

    /// The most recently started live HTTP forward's URL, for "open last forward".
    pub fn last_url(&self) -> Option<String> {
        // Live HTTP forwards only: a failed bind's port may belong to another process, and
        // `http://…:5432` isn't worth opening.
        self.forwards
            .iter()
            .filter(|(_, f)| {
                f.local_port != 0 && f.target.ephemeral.is_none() && f.target.http && f.is_live()
            })
            .max_by_key(|(id, _)| **id)
            .map(|(_, f)| f.url())
    }
}

pub fn human_bytes(bytes: u64) -> String {
    const UNITS: [&str; 4] = ["B", "KB", "MB", "GB"];
    let mut value = bytes as f64;
    let mut unit = 0;
    while value >= 1024.0 && unit < UNITS.len() - 1 {
        value /= 1024.0;
        unit += 1;
    }
    if unit == 0 {
        format!("{bytes} B")
    } else {
        format!("{value:.1} {}", UNITS[unit])
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ipv6_hosts_are_bracketed() {
        assert_eq!(host_port(&display_host("::1"), 80), "[::1]:80");
        assert_eq!(host_port(&display_host("::"), 80), "localhost:80");
        assert_eq!(host_port(&display_host("0.0.0.0"), 80), "localhost:80");
        assert_eq!(host_port(&display_host("127.0.0.1"), 80), "127.0.0.1:80");
        assert_eq!(host_port("fe80::1", 1), "[fe80::1]:1");
    }

    #[test]
    fn human_bytes_scales_units() {
        assert_eq!(human_bytes(500), "500 B");
        assert_eq!(human_bytes(2048), "2.0 KB");
        assert_eq!(human_bytes(5 * 1024 * 1024), "5.0 MB");
    }
}

#[cfg(test)]
mod reach_tests {
    use kubyl_base::host::TestHost;

    use super::*;
    use crate::reach::{self, Reach as _};

    fn request(runtime: &tokio::runtime::Runtime, timeout: Duration) -> ReachRequest {
        // A client for a cluster that isn't there; its buffer needs a runtime.
        let _guard = runtime.enter();
        let client =
            kube::Client::try_from(kube::Config::new("http://127.0.0.1:1".parse().unwrap()))
                .expect("client");
        ReachRequest {
            client,
            cluster: ClusterId::new("c"),
            namespace: "monitoring".into(),
            service: "alertmanager-main".into(),
            port: ReachPort::Number(9093),
            title: "Alertmanager · svc/alertmanager-main".into(),
            https: false,
            timeout,
            reconnect_grace: None,
        }
    }

    /// Opens a request through the channel and returns what the owner receives.
    fn open(
        runtime: &tokio::runtime::Runtime,
        reach: &reach::ChannelReach,
        request: ReachRequest,
    ) -> std::sync::mpsc::Receiver<Result<reach::Reached, String>> {
        let (tx, rx) = std::sync::mpsc::channel();
        let future = reach.reach(request);
        runtime.spawn(async move {
            tx.send(future.await).ok();
        });
        rx
    }

    fn serve(
        core: &mut ForwardsCore,
        host: &mut TestHost<ForwardsCore>,
    ) -> (reach::ChannelReach, TaskHandle) {
        let (reach, inbox) = reach::channel();
        let task = core.serve(inbox, host);
        (reach, task)
    }

    fn started(core: &ForwardsCore) -> u64 {
        core.forwards().map(|(id, _)| id).next().expect("a forward")
    }

    #[test]
    fn a_request_gets_its_port_once_the_forward_listens_and_stops_it_when_dropped() {
        let runtime = tokio::runtime::Runtime::new().unwrap();
        let mut host = TestHost::default();
        let mut core = ForwardsCore::default();
        let (reach, _serving) = serve(&mut core, &mut host);
        let answer = open(&runtime, &reach, request(&runtime, Duration::from_secs(30)));

        host.run_until(&mut core, |core, _| core.forwards().count() == 1);
        let id = started(&core);
        assert!(matches!(
            host.effects.as_slice(),
            [.., ForwardsEffect::Reaching(opened)] if *opened == id
        ));
        let target = core.forward(id).unwrap().target();
        assert_eq!(target.bind_address, "127.0.0.1");
        assert_eq!(
            target.ephemeral.as_deref(),
            Some("Alertmanager · svc/alertmanager-main")
        );
        assert!(core.active().is_empty(), "temporary forwards aren't listed");

        // The listener says it listens.
        core.forwards.get_mut(&id).unwrap().state = ForwardState::Starting;
        core.apply_event(
            id,
            &ForwardEvent::Listening { local_port: 40123 },
            &mut host,
        );
        let mut reached = answer
            .recv_timeout(Duration::from_secs(10))
            .unwrap()
            .unwrap();
        assert_eq!(reached.local_port, 40123);

        // The user stops it: the feature hears about it.
        let stopped = reached.take_stopped();
        core.remove(id, &mut host);
        runtime.block_on(stopped);

        // Dropping what the feature got stops its forward.
        let answer = open(&runtime, &reach, request(&runtime, Duration::from_secs(30)));
        host.run_until(&mut core, |core, _| core.forwards().count() == 1);
        let id = started(&core);
        core.forwards.get_mut(&id).unwrap().state = ForwardState::Starting;
        core.apply_event(
            id,
            &ForwardEvent::Listening { local_port: 40124 },
            &mut host,
        );
        drop(
            answer
                .recv_timeout(Duration::from_secs(10))
                .unwrap()
                .unwrap(),
        );
        host.run_until(&mut core, |core, _| core.forwards().count() == 0);
        assert!(
            host.effects
                .iter()
                .any(|e| matches!(e, ForwardsEffect::Released(released) if *released == id))
        );
    }

    #[test]
    fn a_forward_that_cannot_bind_fails_the_request() {
        let runtime = tokio::runtime::Runtime::new().unwrap();
        let mut host = TestHost::default();
        let mut core = ForwardsCore::default();
        let (reach, _serving) = serve(&mut core, &mut host);
        let answer = open(&runtime, &reach, request(&runtime, Duration::from_secs(30)));

        host.run_until(&mut core, |core, _| core.forwards().count() == 1);
        let id = started(&core);
        core.set_state(id, ForwardState::Failed("address in use".into()), &mut host);
        let error = answer
            .recv_timeout(Duration::from_secs(10))
            .unwrap()
            .err()
            .unwrap();
        assert_eq!(error, "port-forward: address in use");
        assert_eq!(core.forwards().count(), 0);
    }

    /// A forward that never reports back: no listener events, no probes.
    fn silence(core: &mut ForwardsCore, id: u64, state: ForwardState) {
        let forward = core.forwards.get_mut(&id).unwrap();
        forward._tasks.clear();
        forward.probe = None;
        forward.state = state;
    }

    #[test]
    fn a_forward_that_does_not_listen_in_time_fails_the_request() {
        let runtime = tokio::runtime::Runtime::new().unwrap();
        let mut host = TestHost::default();
        let mut core = ForwardsCore::default();
        let (reach, _serving) = serve(&mut core, &mut host);
        let answer = open(
            &runtime,
            &reach,
            request(&runtime, Duration::from_millis(100)),
        );

        host.run_until(&mut core, |core, _| core.forwards().count() == 1);
        let id = started(&core);
        silence(&mut core, id, ForwardState::Starting);
        host.run_until(&mut core, |core, _| core.forwards().count() == 0);
        let error = answer
            .recv_timeout(Duration::from_secs(10))
            .unwrap()
            .err()
            .unwrap();
        assert_eq!(
            error,
            "the port-forward didn't start (needs create pods/portforward)"
        );
    }

    #[test]
    fn a_probe_that_finds_the_target_again_answers_the_request() {
        let runtime = tokio::runtime::Runtime::new().unwrap();
        let mut host = TestHost::default();
        let mut core = ForwardsCore::default();
        let (reach, _serving) = serve(&mut core, &mut host);
        let answer = open(&runtime, &reach, request(&runtime, Duration::from_secs(30)));

        host.run_until(&mut core, |core, _| core.forwards().count() == 1);
        let id = started(&core);
        // The first lookup failed; the listener comes up meanwhile.
        silence(
            &mut core,
            id,
            ForwardState::Reconnecting("no ready endpoints".into()),
        );
        core.apply_event(
            id,
            &ForwardEvent::Listening { local_port: 40125 },
            &mut host,
        );
        // The next probe finds a ready pod.
        let resolved = resolve::Resolved {
            pod: "alertmanager-main-0".into(),
            port: 9093,
        };
        core.finish_check(id, Ok(resolved), &mut host);
        let reached = answer
            .recv_timeout(Duration::from_secs(10))
            .unwrap()
            .unwrap();
        assert_eq!(reached.local_port, 40125);
    }

    #[test]
    fn a_forward_that_keeps_reconnecting_fails_the_request_after_the_grace_period() {
        let runtime = tokio::runtime::Runtime::new().unwrap();
        let mut host = TestHost::default();
        let mut core = ForwardsCore::default();
        let (reach, _serving) = serve(&mut core, &mut host);
        let mut grace = request(&runtime, Duration::from_secs(60));
        grace.reconnect_grace = Some(Duration::from_millis(100));
        let answer = open(&runtime, &reach, grace);

        host.run_until(&mut core, |core, _| core.forwards().count() == 1);
        let id = started(&core);
        silence(&mut core, id, ForwardState::Starting);
        core.set_state(
            id,
            ForwardState::Reconnecting("no ready endpoints".into()),
            &mut host,
        );
        host.run_until(&mut core, |core, _| core.forwards().count() == 0);
        let error = answer
            .recv_timeout(Duration::from_secs(10))
            .unwrap()
            .err()
            .unwrap();
        assert_eq!(error, "port-forward: no ready endpoints");
    }

    #[test]
    fn named_ports_are_looked_up_before_the_host_hears_about_the_request() {
        let runtime = tokio::runtime::Runtime::new().unwrap();
        let mut host = TestHost::default();
        let mut core = ForwardsCore::default();
        let (reach, _serving) = serve(&mut core, &mut host);
        let mut named = request(&runtime, Duration::from_secs(1));
        named.port = ReachPort::Name("web".into());
        // The Service can't be read (nothing listens), so there is no such port.
        let answer = open(&runtime, &reach, named);
        let error = answer
            .recv_timeout(Duration::from_secs(10))
            .unwrap()
            .err()
            .unwrap();
        assert_eq!(error, "svc/alertmanager-main has no port web");
        assert_eq!(core.forwards().count(), 0);
    }
}
