//! The port-forward manager: starts/stops forwards, tracks their state (listening, reconnecting,
//! failed), connections and bytes, and shows them in `kubyl_logs`'s Active Sessions panel with
//! buttons to open the forward in a browser, copy its address and save it.
//!
//! A forward whose target stops resolving (the pod died, the Service has no ready endpoints) is
//! "reconnecting": the local port stays open and a probe re-resolves the target every few
//! seconds until a pod answers again. New connections always resolve afresh, so a Service
//! forward survives pod restarts.

use std::collections::HashMap;
use std::fmt;
use std::sync::Arc;
use std::time::Duration;

use futures::StreamExt as _;
use futures::channel::mpsc;
use gpui::{App, AppContext as _, ClipboardItem, Context, Entity, EventEmitter, Global, Task};
use kubyl_core::forwards::ActiveForward;
use kubyl_core::{ClusterId, Notification, NotificationCenter, ResourceRef, Tone};
use kubyl_logs::sessions::{SessionButton, SessionId, SessionKind, SessionRegistry};
use kubyl_ui::IconName;

use crate::favorites::{SavedForward, SavedForwards, short_kind};
use crate::listener::{self, ForwardEvent};
use crate::resolve::{self, ForwardKind, RemotePort};

/// How often a reconnecting forward re-resolves its target.
const PROBE_INTERVAL: Duration = Duration::from_secs(4);

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct ForwardId(u64);

#[derive(Clone, Debug)]
pub struct ForwardSpec {
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
    /// A temporary forward owned by another feature (web views): never saved, not listed in
    /// [`kubyl_core::forwards::ActiveForwards`].
    pub ephemeral: Option<Ephemeral>,
}

/// How a temporary forward shows in Active Sessions, and who to tell when the user stops it.
#[derive(Clone)]
pub struct Ephemeral {
    /// The session title, e.g. `web view · svc/grafana :3000`.
    pub title: String,
    /// Extra buttons on the session row (before the stop button).
    pub buttons: Vec<SessionButton>,
    /// Called after the user stopped the forward from Active Sessions.
    pub on_stop: Arc<dyn Fn(&mut App)>,
}

impl fmt::Debug for Ephemeral {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("Ephemeral")
            .field("title", &self.title)
            .finish_non_exhaustive()
    }
}

impl ForwardSpec {
    /// `svc/ledger :5432`, or the title of a temporary forward.
    pub fn session_label(&self) -> String {
        match &self.ephemeral {
            Some(ephemeral) => ephemeral.title.clone(),
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

struct Forward {
    spec: ForwardSpec,
    state: ForwardState,
    local_port: u16,
    /// The remote port of the last resolution.
    resolved_port: Option<u16>,
    /// The pod of the last resolution or connection.
    resolved_pod: Option<String>,
    connections: u64,
    bytes_sent: u64,
    bytes_received: u64,
    session_id: SessionId,
    saved: bool,
    /// The client the forward was started with (probes use it too).
    client: kube::Client,
    _task: Task<()>,
    probe: Option<Task<()>>,
}

impl Forward {
    fn url(&self) -> String {
        let scheme = if self.spec.https { "https" } else { "http" };
        let host = match self.spec.bind_address.as_str() {
            "0.0.0.0" | "::" | "" => "localhost",
            other => other,
        };
        format!("{scheme}://{host}:{}", self.local_port)
    }

    fn address(&self) -> String {
        format!("{}:{}", self.spec.bind_address, self.local_port)
    }

    /// `svc/ledger :5432 → :15432`.
    fn title(&self) -> String {
        let target = self.spec.session_label();
        if self.local_port == 0 {
            target
        } else {
            format!("{target} → :{}", self.local_port)
        }
    }

    fn status(&self) -> (String, Tone) {
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
}

#[derive(Default)]
pub struct PortForwardManager {
    forwards: HashMap<u64, Forward>,
    next_id: u64,
}

impl EventEmitter<()> for PortForwardManager {}

struct GlobalManager(Entity<PortForwardManager>);

impl Global for GlobalManager {}

impl PortForwardManager {
    pub fn install(cx: &mut App) -> Entity<Self> {
        let entity = cx.new(|_| Self::default());
        cx.set_global(GlobalManager(entity.clone()));
        entity
    }

    pub fn global(cx: &App) -> Entity<Self> {
        cx.global::<GlobalManager>().0.clone()
    }

    /// Starts a forward and returns its id.
    pub fn start(client: kube::Client, spec: ForwardSpec, cx: &mut App) -> ForwardId {
        let manager = Self::global(cx);
        let id = manager.update(cx, |this, _| {
            let id = this.next_id;
            this.next_id += 1;
            id
        });

        let (events_tx, mut events_rx) = mpsc::unbounded();
        let (namespace, kind, port, bind_address, local_port) = (
            spec.namespace.clone(),
            spec.kind.clone(),
            spec.port.clone(),
            spec.bind_address.clone(),
            spec.local_port,
        );
        let listen_client = client.clone();
        let task_manager = manager.clone();
        let task = cx.spawn(async move |cx| {
            let run = cx.update(|cx| {
                kubyl_core::spawn_kube(cx, async move {
                    listener::run(
                        listen_client,
                        namespace,
                        kind,
                        port,
                        bind_address,
                        local_port,
                        events_tx,
                    )
                    .await
                })
            });
            while let Some(mut event) = events_rx.next().await {
                // Drain what's already queued (a connection burst) before the pause below.
                loop {
                    let alive = cx.update(|cx| {
                        task_manager.update(cx, |this, cx| this.apply_event(id, &event, cx))
                    });
                    if !alive {
                        return;
                    }
                    match events_rx.try_recv() {
                        Ok(next) => event = next,
                        Err(_) => break,
                    }
                }
                cx.background_executor()
                    .timer(Duration::from_millis(100))
                    .await;
            }
            // The listener returned: binding the local port failed.
            if let Err(err) = run.await {
                cx.update(|cx| {
                    task_manager.update(cx, |this, cx| {
                        this.set_state(id, ForwardState::Failed(format!("{err:#}")), cx)
                    });
                    NotificationCenter::push(
                        cx,
                        Notification::error(format!("Port-forward failed: {err:#}")),
                    );
                });
            }
        });

        let saved = spec.ephemeral.is_none()
            && SavedForwards::global(cx)
                .read(cx)
                .state()
                .contains(&saved_forward(&spec, cx));
        manager.update(cx, |this, cx| {
            let session_id = SessionRegistry::add(
                cx,
                SessionKind::PortForward,
                spec.session_label(),
                spec.namespace.clone(),
                "starting…",
                Tone::Info,
                move |cx| {
                    Self::global(cx).update(cx, |this, cx| this.stopped_by_user(id, cx));
                },
            );
            this.forwards.insert(
                id,
                Forward {
                    spec,
                    state: ForwardState::Starting,
                    local_port,
                    resolved_port: None,
                    resolved_pod: None,
                    connections: 0,
                    bytes_sent: 0,
                    bytes_received: 0,
                    session_id,
                    saved,
                    client: client.clone(),
                    _task: task,
                    probe: None,
                },
            );
            this.check_target(id, client, cx);
            this.refresh_row(id, cx);
            cx.emit(());
            cx.notify();
        });
        ForwardId(id)
    }

    /// Stops a forward and removes its session row.
    pub fn stop(&mut self, id: ForwardId, cx: &mut Context<Self>) {
        if let Some(forward) = self.forwards.get(&id.0) {
            SessionRegistry::remove(cx, forward.session_id);
        }
        self.remove(id.0, cx);
    }

    /// Drops a forward (its session row was removed already).
    fn remove(&mut self, id: u64, cx: &mut Context<Self>) {
        if self.forwards.remove(&id).is_some() {
            cx.emit(());
            cx.notify();
        }
    }

    /// The user stopped the forward from Active Sessions: drop it and tell a temporary
    /// forward's owner.
    fn stopped_by_user(&mut self, id: u64, cx: &mut Context<Self>) {
        let on_stop = self
            .forwards
            .get(&id)
            .and_then(|f| f.spec.ephemeral.as_ref())
            .map(|e| e.on_stop.clone());
        self.remove(id, cx);
        if let Some(on_stop) = on_stop {
            cx.defer(move |cx| on_stop(cx));
        }
    }

    /// A forward's state, or `None` once it stopped.
    pub fn info(&self, id: ForwardId) -> Option<ForwardInfo> {
        let forward = self.forwards.get(&id.0)?;
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

    /// Stops the forward with a raw id from [`kubyl_core::forwards::ActiveForward::id`].
    pub fn stop_raw(&mut self, id: u64, cx: &mut Context<Self>) {
        self.stop(ForwardId(id), cx);
    }

    /// The running forwards as other crates see them
    /// ([`kubyl_core::forwards::ActiveForwards`]).
    pub fn active(&self) -> Vec<ActiveForward> {
        let mut forwards: Vec<ActiveForward> = self
            .forwards
            .iter()
            .filter(|(_, forward)| forward.spec.ephemeral.is_none())
            .map(|(id, forward)| {
                let listening = forward.state == ForwardState::Listening && forward.local_port != 0;
                ActiveForward {
                    id: *id,
                    target: forward.spec.target.clone(),
                    remote_port: forward.spec.remote_port,
                    local: listening.then(|| format!("localhost:{}", forward.local_port)),
                    url: (listening && forward.spec.http).then(|| forward.url()),
                }
            })
            .collect();
        forwards.sort_by_key(|f| f.id);
        forwards
    }

    /// Whether a forward to the same target and ports is running.
    pub fn is_running(&self, saved: &SavedForward, cx: &App) -> bool {
        self.forwards
            .values()
            .filter(|f| f.spec.ephemeral.is_none())
            .any(|f| saved_forward(&f.spec, cx).same_target(saved))
    }

    /// Resolves the target once (off the UI thread), to show the remote port and to report a
    /// target that doesn't resolve before the first connection. Failing resolutions put the
    /// forward into "reconnecting" and keep probing.
    fn check_target(&mut self, id: u64, client: kube::Client, cx: &mut Context<Self>) {
        let Some(forward) = self.forwards.get_mut(&id) else {
            return;
        };
        let (namespace, kind, port) = (
            forward.spec.namespace.clone(),
            forward.spec.kind.clone(),
            forward.spec.port.clone(),
        );
        forward.probe = Some(cx.spawn(async move |this, cx| {
            loop {
                let (namespace, kind, port, client) = (
                    namespace.clone(),
                    kind.clone(),
                    port.clone(),
                    client.clone(),
                );
                let result = cx
                    .update(|cx| {
                        kubyl_core::spawn_kube(cx, async move {
                            resolve::resolve(client, &namespace, &kind, port).await
                        })
                    })
                    .await;
                let resolved = this
                    .update(cx, |this, cx| {
                        let Some(forward) = this.forwards.get_mut(&id) else {
                            return true;
                        };
                        match result {
                            Ok(resolved) => {
                                forward.resolved_port = Some(resolved.port);
                                forward.resolved_pod = Some(resolved.pod);
                                if matches!(forward.state, ForwardState::Reconnecting(_)) {
                                    forward.state = ForwardState::Listening;
                                }
                                // The loop ends here: a later error must be able to probe
                                // again. (Dropping this task from inside is fine, it returns
                                // right after.)
                                forward.probe = None;
                                this.refresh_row(id, cx);
                                true
                            }
                            // Binding the local port failed: nothing to reconnect.
                            Err(_) if matches!(forward.state, ForwardState::Failed(_)) => {
                                forward.probe = None;
                                true
                            }
                            Err(err) => {
                                forward.state = ForwardState::Reconnecting(format!("{err:#}"));
                                this.refresh_row(id, cx);
                                false
                            }
                        }
                    })
                    .unwrap_or(true);
                if resolved {
                    break;
                }
                cx.background_executor().timer(PROBE_INTERVAL).await;
            }
        }));
    }

    fn set_state(&mut self, id: u64, state: ForwardState, cx: &mut Context<Self>) {
        if let Some(forward) = self.forwards.get_mut(&id) {
            forward.state = state;
            self.refresh_row(id, cx);
        }
    }

    fn apply_event(&mut self, id: u64, event: &ForwardEvent, cx: &mut Context<Self>) -> bool {
        let Some(forward) = self.forwards.get_mut(&id) else {
            return false;
        };
        match event {
            ForwardEvent::Listening { local_port } => {
                forward.local_port = *local_port;
                if forward.state == ForwardState::Starting {
                    forward.state = ForwardState::Listening;
                }
                if forward.spec.open_browser && forward.spec.http {
                    cx.open_url(&forward.url());
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
                tracing::debug!(forward = %forward.spec.session_label(), "port-forward error: {message}");
                if !matches!(forward.state, ForwardState::Failed(_)) {
                    forward.state = ForwardState::Reconnecting(message.clone());
                    if forward.probe.is_none() {
                        let client = forward.client.clone();
                        self.check_target(id, client, cx);
                    }
                }
            }
        }
        self.refresh_row(id, cx);
        true
    }

    /// Pushes title, status and buttons to the Active Sessions row.
    fn refresh_row(&mut self, id: u64, cx: &mut Context<Self>) {
        let Some(forward) = self.forwards.get(&id) else {
            return;
        };
        let (status, tone) = forward.status();
        let session = forward.session_id;
        let title = forward.title();
        let mut buttons = Vec::new();
        if matches!(
            forward.state,
            ForwardState::Listening | ForwardState::Reconnecting(_)
        ) {
            if forward.spec.http {
                let url = forward.url();
                buttons.push(SessionButton::new(
                    "open",
                    IconName::Globe,
                    format!("Open {url}"),
                    move |_, cx| cx.open_url(&url),
                ));
            }
            let address = forward.address();
            buttons.push(SessionButton::new(
                "copy",
                IconName::Copy,
                format!("Copy {address}"),
                move |_, cx| cx.write_to_clipboard(ClipboardItem::new_string(address.clone())),
            ));
        }
        let saved = forward.saved;
        if let Some(ephemeral) = &forward.spec.ephemeral {
            buttons.extend(ephemeral.buttons.iter().cloned());
            SessionRegistry::set_title(cx, session, title);
            SessionRegistry::set_status(cx, session, status, tone);
            SessionRegistry::set_buttons(cx, session, buttons);
            cx.notify();
            return;
        }
        buttons.push(SessionButton::new(
            "save",
            if saved {
                IconName::StarFilled
            } else {
                IconName::Star
            },
            if saved {
                "Forget this forward"
            } else {
                "Save this forward"
            },
            move |_, cx| {
                Self::global(cx).update(cx, |this, cx| this.toggle_saved(id, cx));
            },
        ));
        SessionRegistry::set_title(cx, session, title);
        SessionRegistry::set_status(cx, session, status, tone);
        SessionRegistry::set_buttons(cx, session, buttons);
        cx.notify();
    }

    fn toggle_saved(&mut self, id: u64, cx: &mut Context<Self>) {
        let Some(forward) = self.forwards.get_mut(&id) else {
            return;
        };
        if forward.spec.ephemeral.is_some() {
            return;
        }
        let saved = saved_forward(&forward.spec, cx);
        forward.saved = !forward.saved;
        let keep = forward.saved;
        SavedForwards::global(cx).update(cx, |this, cx| {
            this.update_state(cx, |state| {
                if keep {
                    state.upsert(saved);
                } else {
                    state.remove(&saved);
                }
            })
        });
        self.refresh_row(id, cx);
    }

    /// The most recently started live HTTP forward's URL, for "open last forward".
    pub fn last_url(&self) -> Option<String> {
        // Live HTTP forwards only: a failed bind's port may belong to another process, and
        // `http://…:5432` isn't worth opening.
        self.forwards
            .iter()
            .filter(|(_, f)| {
                f.local_port != 0
                    && f.spec.ephemeral.is_none()
                    && f.spec.http
                    && matches!(
                        f.state,
                        ForwardState::Listening | ForwardState::Reconnecting(_)
                    )
            })
            .max_by_key(|(id, _)| **id)
            .map(|(_, f)| f.url())
    }
}

/// The saved-forward form of a spec (its context resolved through the connection manager).
pub fn saved_forward(spec: &ForwardSpec, cx: &App) -> SavedForward {
    let context = kubyl_kube::ConnectionManager::try_global(cx)
        .and_then(|manager| manager.read(cx).context(&spec.cluster).cloned());
    SavedForward {
        context: context
            .as_ref()
            .map(|c| c.context.clone())
            .unwrap_or_default(),
        server: context.as_ref().and_then(|c| c.server.clone()),
        file: context.map(|c| c.file).unwrap_or_default(),
        namespace: spec.namespace.clone(),
        resource: spec.target.gvr.resource.clone(),
        name: spec.target.name.clone().unwrap_or_default(),
        remote_port: spec.remote_port,
        local_port: spec.local_port,
        bind_address: spec.bind_address.clone(),
        auto_start: false,
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
    fn human_bytes_scales_units() {
        assert_eq!(human_bytes(500), "500 B");
        assert_eq!(human_bytes(2048), "2.0 KB");
        assert_eq!(human_bytes(5 * 1024 * 1024), "5.0 MB");
    }
}
