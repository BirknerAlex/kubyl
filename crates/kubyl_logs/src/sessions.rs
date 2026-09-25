//! The active-sessions registry: log streams, terminals and port-forwards that any crate can
//! list in one right-dock panel plus status-bar counters.
//!
//! `kubyl_logs` owns this global because it initializes first (see `crates/kubyl/src/main.rs`).
//! `kubyl_terminal` and `kubyl_portforward` depend on `kubyl_logs` and push/update/remove their
//! own sessions into it; they never read each other's session lists directly.

use std::collections::HashMap;
use std::sync::Arc;
use std::time::Instant;

use gpui::{App, AppContext as _, Entity, EventEmitter, Global, SharedString, Window};
use kubyl_core::Tone;
use kubyl_ui::IconName;

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
    /// Where it runs (namespace, cluster), shown before the status.
    pub subtitle: SharedString,
    /// Live details: `3 pods · follow · 42 lines/s`, `2 conns · 1.2 MB`, `reconnecting…`.
    pub status: SharedString,
    pub tone: Tone,
    pub started: Instant,
    /// Extra buttons shown before the stop button (open in browser, save as favorite…).
    pub buttons: Vec<SessionButton>,
}

/// What a session row button does.
pub type ButtonHandler = Arc<dyn Fn(&mut Window, &mut App)>;

/// A button on a session row.
#[derive(Clone)]
pub struct SessionButton {
    pub id: &'static str,
    pub icon: IconName,
    pub tooltip: SharedString,
    pub on_click: ButtonHandler,
}

impl SessionButton {
    pub fn new(
        id: &'static str,
        icon: IconName,
        tooltip: impl Into<SharedString>,
        on_click: impl Fn(&mut Window, &mut App) + 'static,
    ) -> Self {
        Self {
            id,
            icon,
            tooltip: tooltip.into(),
            on_click: Arc::new(on_click),
        }
    }
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
                started: Instant::now(),
                buttons: Vec::new(),
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

    /// Changes a session's title (a shell whose container or shell was resolved…).
    pub fn set_title(cx: &mut App, id: SessionId, title: impl Into<SharedString>) {
        let registry = Self::global(cx);
        registry.update(cx, |this, cx| {
            if let Some(session) = this.sessions.iter_mut().find(|s| s.id == id) {
                session.title = title.into();
                cx.emit(());
                cx.notify();
            }
        });
    }

    /// Replaces the extra buttons of a session row.
    pub fn set_buttons(cx: &mut App, id: SessionId, buttons: Vec<SessionButton>) {
        let registry = Self::global(cx);
        registry.update(cx, |this, cx| {
            if let Some(session) = this.sessions.iter_mut().find(|s| s.id == id) {
                session.buttons = buttons;
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

    pub fn get(&self, id: SessionId) -> Option<&SessionInfo> {
        self.sessions.iter().find(|s| s.id == id)
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
            SessionRegistry::set_title(cx, id, "web-1");
            SessionRegistry::set_buttons(
                cx,
                id,
                vec![SessionButton::new(
                    "open",
                    IconName::Globe,
                    "Open",
                    |_, _| {},
                )],
            );
            let session = registry.read(cx).get(id).unwrap();
            assert_eq!(session.title.as_ref(), "web-1");
            assert_eq!(session.buttons.len(), 1);
            SessionRegistry::stop(cx, id);
            assert!(stopped.load(Ordering::SeqCst));
            assert_eq!(registry.read(cx).all().len(), 0);
        });
    }
}
