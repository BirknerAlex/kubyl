//! The active-sessions registry: log streams, terminals and port-forwards that any crate can
//! list in one right-dock panel plus status-bar counters.
//!
//! `kubyl_logs` owns this global because it initializes first (see `crates/kubyl/src/main.rs`).
//! `kubyl_terminal` and `kubyl_portforward` depend on `kubyl_logs` and push/update/remove their
//! own sessions into it; they never read each other's session lists directly.

use std::collections::HashMap;
use std::sync::Arc;

use gpui::{App, AppContext as _, Entity, EventEmitter, Global, SharedString};
use kubyl_core::Tone;

/// Identifies one row in the active-sessions panel.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct SessionId(u64);

impl SessionId {
    /// A stable numeric handle, for use as an `ElementId`.
    pub fn raw(self) -> u64 {
        self.0
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SessionKind {
    Logs,
    Terminal,
    PortForward,
}

impl SessionKind {
    pub fn label(self) -> &'static str {
        match self {
            SessionKind::Logs => "Logs",
            SessionKind::Terminal => "Terminal",
            SessionKind::PortForward => "Port-forward",
        }
    }
}

#[derive(Clone)]
pub struct SessionInfo {
    pub id: SessionId,
    pub kind: SessionKind,
    pub title: SharedString,
    pub subtitle: SharedString,
    pub status: SharedString,
    pub tone: Tone,
}

type StopFn = Arc<dyn Fn(&mut App) + Send + Sync>;

/// Every active session, plus the callback that stops each one.
#[derive(Default)]
pub struct SessionRegistry {
    sessions: Vec<SessionInfo>,
    stops: HashMap<SessionId, StopFn>,
    next_id: u64,
}

impl EventEmitter<()> for SessionRegistry {}

struct GlobalSessions(Entity<SessionRegistry>);

impl Global for GlobalSessions {}

impl SessionRegistry {
    pub fn install(cx: &mut App) -> Entity<Self> {
        let entity = cx.new(|_| Self::default());
        cx.set_global(GlobalSessions(entity.clone()));
        entity
    }

    pub fn global(cx: &App) -> Entity<Self> {
        cx.global::<GlobalSessions>().0.clone()
    }

    /// Registers a new session and returns its id. `on_stop` is called (and then the session is
    /// removed) when the user clicks "stop" in the panel.
    pub fn add(
        cx: &mut App,
        kind: SessionKind,
        title: impl Into<SharedString>,
        subtitle: impl Into<SharedString>,
        status: impl Into<SharedString>,
        tone: Tone,
        on_stop: impl Fn(&mut App) + Send + Sync + 'static,
    ) -> SessionId {
        let registry = Self::global(cx);
        registry.update(cx, |this, cx| {
            let id = SessionId(this.next_id);
            this.next_id += 1;
            this.sessions.push(SessionInfo {
                id,
                kind,
                title: title.into(),
                subtitle: subtitle.into(),
                status: status.into(),
                tone,
            });
            this.stops.insert(id, Arc::new(on_stop));
            cx.emit(());
            cx.notify();
            id
        })
    }

    /// Updates the status label/tone of a session (e.g. connection counts, reconnecting…).
    pub fn set_status(cx: &mut App, id: SessionId, status: impl Into<SharedString>, tone: Tone) {
        let registry = Self::global(cx);
        registry.update(cx, |this, cx| {
            if let Some(session) = this.sessions.iter_mut().find(|s| s.id == id) {
                session.status = status.into();
                session.tone = tone;
                cx.emit(());
                cx.notify();
            }
        });
    }

    /// Removes a session without calling its stop callback (the underlying work already ended,
    /// e.g. the view was dropped).
    pub fn remove(cx: &mut App, id: SessionId) {
        let registry = Self::global(cx);
        registry.update(cx, |this, cx| {
            this.sessions.retain(|s| s.id != id);
            this.stops.remove(&id);
            cx.emit(());
            cx.notify();
        });
    }

    /// Calls the session's stop callback, then removes it.
    pub fn stop(cx: &mut App, id: SessionId) {
        let registry = Self::global(cx);
        let stop = registry.update(cx, |this, _| this.stops.remove(&id));
        if let Some(stop) = stop {
            stop(cx);
        }
        Self::remove(cx, id);
    }

    pub fn all(&self) -> &[SessionInfo] {
        &self.sessions
    }

    pub fn count(&self, kind: SessionKind) -> usize {
        self.sessions.iter().filter(|s| s.kind == kind).count()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::{AtomicBool, Ordering};

    #[gpui::test]
    fn add_stop_and_count(cx: &mut gpui::TestAppContext) {
        cx.update(|cx| {
            SessionRegistry::install(cx);
            let stopped = Arc::new(AtomicBool::new(false));
            let flag = stopped.clone();
            let id = SessionRegistry::add(
                cx,
                SessionKind::Logs,
                "web-0",
                "default",
                "streaming",
                Tone::Good,
                move |_| flag.store(true, Ordering::SeqCst),
            );
            let registry = SessionRegistry::global(cx);
            assert_eq!(registry.read(cx).count(SessionKind::Logs), 1);
            SessionRegistry::set_status(cx, id, "reconnecting", Tone::Warning);
            assert_eq!(registry.read(cx).all()[0].status.as_ref(), "reconnecting");
            SessionRegistry::stop(cx, id);
            assert!(stopped.load(Ordering::SeqCst));
            assert_eq!(registry.read(cx).all().len(), 0);
        });
    }
}
