//! The port-forward manager: starts/stops forwards and tracks their connection/byte counters,
//! surfaced through `kubyl_logs`'s shared active-sessions registry (list, status, stop) and its
//! status-bar counters.

use std::collections::HashMap;

use futures::StreamExt as _;
use futures::channel::mpsc;
use gpui::{App, AppContext as _, Context, Entity, EventEmitter, Global, Task};
use kubyl_core::{ClusterId, Tone};
use kubyl_logs::sessions::{SessionId, SessionKind, SessionRegistry};

use crate::listener::{self, ForwardEvent};
use crate::resolve::{ForwardKind, RemotePort};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct ForwardId(u64);

#[derive(Clone)]
pub struct ForwardSpec {
    pub cluster: ClusterId,
    pub namespace: String,
    pub kind: ForwardKind,
    pub port: RemotePort,
    pub bind_address: String,
    /// `0` = pick a free port.
    pub local_port: u16,
    pub label: String,
}

struct Forward {
    label: String,
    bind_address: String,
    local_port: u16,
    listening: bool,
    connections: u64,
    bytes_sent: u64,
    bytes_received: u64,
    session_id: SessionId,
    _task: Task<()>,
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

        let (events_tx, events_rx) = mpsc::unbounded();
        let namespace = spec.namespace.clone();
        let kind = spec.kind.clone();
        let port = spec.port.clone();
        let bind_address = spec.bind_address.clone();
        let local_port = spec.local_port;
        let task_manager = manager.clone();
        let task = cx.spawn(async move |cx| {
            let run_task = cx.update(|cx| {
                kubyl_core::spawn_kube(cx, async move {
                    listener::run(
                        client,
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
            let mut events_rx = events_rx;
            while let Some(mut event) = events_rx.next().await {
                // Drain any other events already sitting in the channel (a connection burst can
                // queue many before we get scheduled again) so throughput isn't capped by the
                // post-batch sleep below.
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
                // Batches status-bar updates so a busy forward doesn't re-render per byte.
                cx.background_executor()
                    .timer(std::time::Duration::from_millis(100))
                    .await;
            }
            // The events channel closed because `listener::run` returned — normally only when
            // binding the local port failed, since its accept loop otherwise runs forever.
            // Surface that error instead of leaving the session stuck at "starting" forever.
            if let Err(err) = run_task.await {
                cx.update(|cx| {
                    task_manager.update(cx, |this, cx| {
                        this.apply_event(id, &ForwardEvent::Error(err.to_string()), cx)
                    });
                });
            }
        });

        manager.update(cx, |this, cx| {
            let session_id = SessionRegistry::add(
                cx,
                SessionKind::PortForward,
                spec.label.clone(),
                format!("{}:{} → starting…", spec.bind_address, spec.local_port),
                "starting",
                Tone::Neutral,
                move |cx| {
                    let manager = Self::global(cx);
                    manager.update(cx, |this, cx| this.stop_without_session(id, cx));
                },
            );
            this.forwards.insert(
                id,
                Forward {
                    label: spec.label,
                    bind_address: spec.bind_address,
                    local_port: spec.local_port,
                    listening: false,
                    connections: 0,
                    bytes_sent: 0,
                    bytes_received: 0,
                    session_id,
                    _task: task,
                },
            );
            cx.emit(());
            cx.notify();
        });
        ForwardId(id)
    }

    /// Stops a forward and removes its session row (for the "stop" button in the panel).
    pub fn stop(&mut self, id: ForwardId, cx: &mut Context<Self>) {
        if let Some(forward) = self.forwards.remove(&id.0) {
            SessionRegistry::remove(cx, forward.session_id);
            cx.emit(());
            cx.notify();
        }
    }

    /// Like [`Self::stop`], but called from the session's own `on_stop` (which already removed
    /// the session row), so it must not remove it again.
    fn stop_without_session(&mut self, id: u64, cx: &mut Context<Self>) {
        if self.forwards.remove(&id).is_some() {
            cx.emit(());
            cx.notify();
        }
    }

    fn apply_event(&mut self, id: u64, event: &ForwardEvent, cx: &mut Context<Self>) -> bool {
        let Some(forward) = self.forwards.get_mut(&id) else {
            return false;
        };
        match event {
            ForwardEvent::Listening { local_port } => {
                forward.local_port = *local_port;
                forward.listening = true;
            }
            ForwardEvent::ConnectionOpened => {
                forward.connections += 1;
            }
            ForwardEvent::ConnectionClosed => {
                forward.connections = forward.connections.saturating_sub(1);
            }
            ForwardEvent::BytesTransferred { sent, received } => {
                forward.bytes_sent += sent;
                forward.bytes_received += received;
            }
            ForwardEvent::Error(message) => {
                SessionRegistry::set_status(
                    cx,
                    forward.session_id,
                    format!("error: {message}"),
                    Tone::Bad,
                );
                tracing::debug!(forward = %forward.label, "port-forward error: {message}");
                cx.notify();
                return true;
            }
        }
        let status = format!(
            "{}:{} · {} conns · {} sent / {} recv",
            forward.bind_address,
            forward.local_port,
            forward.connections,
            human_bytes(forward.bytes_sent),
            human_bytes(forward.bytes_received)
        );
        SessionRegistry::set_status(cx, forward.session_id, status, Tone::Good);
        cx.notify();
        true
    }

    pub fn local_url(&self, id: ForwardId) -> Option<String> {
        self.forwards
            .get(&id.0)
            .map(|f| format!("http://{}:{}", f.bind_address, f.local_port))
    }

    /// The most recently started forward's local URL, for the "open last forward" action.
    /// Only considers forwards that reached `Listening`, so a bind failure never yields
    /// `http://127.0.0.1:0`.
    pub fn last_url(&self) -> Option<String> {
        self.forwards
            .iter()
            .filter(|(_, forward)| forward.listening)
            .max_by_key(|(id, _)| **id)
            .map(|(_, forward)| format!("http://{}:{}", forward.bind_address, forward.local_port))
    }
}

fn human_bytes(bytes: u64) -> String {
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
