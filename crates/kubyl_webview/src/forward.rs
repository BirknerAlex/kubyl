//! Temporary port-forwards behind web views, on phase 05's port-forward manager.
//!
//! - One forward per [`WebTarget`] (service port), shared by every tab showing it
//!   (reference-counted by the tabs that hold it).
//! - Loopback only, on the port the target had last time when that's free (the page keeps its
//!   origin and so its local storage), else a random one. Never saved.
//! - Stops when the last tab lets go (closed, or idle in the background), when the cluster
//!   disconnects, or with the app. Restarts for open tabs when the cluster reconnects.
//! - Shows in Active Sessions as `web view · svc/grafana:80`; stopping it there closes its tabs.

use std::cell::RefCell;
use std::collections::HashMap;
use std::rc::Rc;
use std::sync::Arc;

use gpui::{AnyWeakEntity, App, AppContext as _, Context, Entity, EntityId, Global, WeakEntity};
use kubyl_core::ClusterId;
use kubyl_kube::{ConnectionEvent, ConnectionManager};
use kubyl_portforward::manager::{
    Ephemeral, ForwardId, ForwardInfo, ForwardSpec, ForwardState, PortForwardManager,
};
use kubyl_portforward::resolve::{ForwardKind, RemotePort};

use crate::store::{self, PortKey};
use crate::target::{Scheme, TargetKind, WebTarget};
use crate::view::WebViewTab;

/// What a tab sees of its forward.
#[derive(Clone, Debug, PartialEq)]
pub enum ForwardStatus {
    /// Waiting for the local port.
    Starting,
    /// Listening on `127.0.0.1:<local_port>`.
    Ready { local_port: u16, info: ForwardInfo },
    /// Bind failed, or the forward couldn't start (cluster not connected).
    Failed(String),
    /// Stopped because the cluster disconnected; restarts when it reconnects.
    Disconnected,
}

/// Starts and stops the actual forwards (the port-forward manager; a fake in tests).
pub trait ForwardBackend: 'static {
    /// Starts a forward for `target`; the id is the backend's own.
    fn start(&self, target: &WebTarget, https: bool, cx: &mut App) -> Result<u64, String>;
    fn stop(&self, id: u64, cx: &mut App);
    /// `None` once the forward is gone.
    fn info(&self, id: u64, cx: &App) -> Option<ForwardInfo>;
}

struct Holder {
    /// The tab ([`WebViewTab`] in the app).
    tab: AnyWeakEntity,
    /// The tab wants the forward now (not idle in the background).
    active: bool,
}

struct Entry {
    forward: Option<u64>,
    https: bool,
    holders: Vec<Holder>,
    error: Option<String>,
    disconnected: bool,
    /// The local port is already stored in state.json.
    remembered: bool,
}

impl Entry {
    fn live_holders(&self) -> impl Iterator<Item = &Holder> {
        self.holders.iter().filter(|h| h.tab.upgrade().is_some())
    }

    fn wanted(&self) -> bool {
        self.live_holders().any(|h| h.active)
    }
}

pub struct WebForwards {
    entries: HashMap<WebTarget, Entry>,
    backend: Rc<dyn ForwardBackend>,
}

struct GlobalForwards(Entity<WebForwards>);

impl Global for GlobalForwards {}

impl WebForwards {
    /// Installs the global on the port-forward manager, following cluster connections.
    pub fn install(cx: &mut App) -> Entity<Self> {
        let manager = PortForwardManager::global(cx);
        let entity = Self::install_with(Rc::new(ManagerBackend::default()), cx);
        entity.update(cx, |_, cx| {
            // Forward states (listening, connections) come from the manager.
            cx.observe(&manager, |this: &mut Self, _, cx| {
                this.on_backend_changed(cx)
            })
            .detach();
            if let Some(connections) = ConnectionManager::try_global(cx) {
                cx.subscribe(&connections, |this, connections, event, cx| {
                    if let ConnectionEvent::StateChanged(cluster) = event {
                        let connected = connections.read(cx).state(cluster).is_connected();
                        this.cluster_changed(cluster, connected, cx);
                    }
                })
                .detach();
            }
        });
        entity
    }

    /// Installs the global on `backend` (tests).
    pub fn install_with(backend: Rc<dyn ForwardBackend>, cx: &mut App) -> Entity<Self> {
        let entity = cx.new(|_| Self {
            entries: HashMap::new(),
            backend,
        });
        cx.set_global(GlobalForwards(entity.clone()));
        entity
    }

    pub fn global(cx: &App) -> Entity<Self> {
        cx.global::<GlobalForwards>().0.clone()
    }

    pub fn try_global(cx: &App) -> Option<Entity<Self>> {
        cx.try_global::<GlobalForwards>().map(|g| g.0.clone())
    }

    /// `tab` holds (or, `active = false`, keeps a claim on without running) the forward of
    /// `target`, starting it when needed.
    pub fn hold(
        &mut self,
        target: &WebTarget,
        tab: AnyWeakEntity,
        active: bool,
        scheme: Scheme,
        cx: &mut Context<Self>,
    ) {
        let entry = self.entries.entry(target.clone()).or_insert_with(|| Entry {
            forward: None,
            https: scheme == Scheme::Https,
            holders: Vec::new(),
            error: None,
            disconnected: false,
            remembered: false,
        });
        let id = tab.entity_id();
        match entry.holders.iter_mut().find(|h| h.tab.entity_id() == id) {
            Some(holder) => holder.active = active,
            None => entry.holders.push(Holder { tab, active }),
        }
        self.sync(target, cx);
    }

    /// `tab` lets go of `target` (it closed).
    pub fn release(&mut self, target: &WebTarget, tab: EntityId, cx: &mut Context<Self>) {
        if let Some(entry) = self.entries.get_mut(target) {
            entry.holders.retain(|h| h.tab.entity_id() != tab);
        }
        self.sync(target, cx);
    }

    /// Starts or stops the forward of `target` to match its holders.
    fn sync(&mut self, target: &WebTarget, cx: &mut Context<Self>) {
        let Some(entry) = self.entries.get_mut(target) else {
            return;
        };
        entry.holders.retain(|h| h.tab.upgrade().is_some());
        let wanted = entry.wanted() && !entry.disconnected;
        match (wanted, entry.forward) {
            (true, None) => {
                entry.error = None;
                entry.remembered = false;
                match self.backend.start(target, entry.https, cx) {
                    Ok(id) => entry.forward = Some(id),
                    Err(err) => entry.error = Some(err),
                }
            }
            (false, Some(id)) => {
                entry.forward = None;
                self.backend.stop(id, cx);
            }
            _ => {}
        }
        if entry.holders.is_empty() {
            self.entries.remove(target);
        }
        cx.notify();
    }

    /// The forward's state for `target`.
    pub fn status(&self, target: &WebTarget, cx: &App) -> Option<ForwardStatus> {
        let entry = self.entries.get(target)?;
        if entry.disconnected {
            return Some(ForwardStatus::Disconnected);
        }
        if let Some(error) = &entry.error {
            return Some(ForwardStatus::Failed(error.clone()));
        }
        let info = self.backend.info(entry.forward?, cx)?;
        Some(match &info.state {
            ForwardState::Failed(err) => ForwardStatus::Failed(err.clone()),
            _ if info.local_port == 0 => ForwardStatus::Starting,
            _ => ForwardStatus::Ready {
                local_port: info.local_port,
                info,
            },
        })
    }

    /// How many open tabs show `target`.
    pub fn tab_count(&self, target: &WebTarget) -> usize {
        self.entries
            .get(target)
            .map(|e| e.live_holders().count())
            .unwrap_or_default()
    }

    /// The open tabs of `target`.
    pub fn tabs(&self, target: &WebTarget) -> Vec<Entity<WebViewTab>> {
        self.entries
            .get(target)
            .map(|e| {
                e.live_holders()
                    .filter_map(|h| h.tab.upgrade()?.downcast::<WebViewTab>().ok())
                    .collect()
            })
            .unwrap_or_default()
    }

    /// Targets with a running forward.
    pub fn running(&self) -> impl Iterator<Item = &WebTarget> {
        self.entries
            .iter()
            .filter(|(_, e)| e.forward.is_some())
            .map(|(t, _)| t)
    }

    /// Stops the forward of `target` and closes its tabs.
    pub fn stop_and_close(&mut self, target: &WebTarget, cx: &mut Context<Self>) {
        let Some(entry) = self.entries.remove(target) else {
            return;
        };
        if let Some(id) = entry.forward {
            self.backend.stop(id, cx);
        }
        close_tabs(entry, cx);
        cx.notify();
    }

    /// The user stopped the forward from Active Sessions: the forward is gone already.
    pub(crate) fn stopped_by_user(&mut self, target: &WebTarget, cx: &mut Context<Self>) {
        if let Some(entry) = self.entries.remove(target) {
            close_tabs(entry, cx);
        }
        cx.notify();
    }

    /// A cluster (dis)connected: stop its forwards, or restart them for open tabs.
    pub fn cluster_changed(
        &mut self,
        cluster: &ClusterId,
        connected: bool,
        cx: &mut Context<Self>,
    ) {
        let targets: Vec<WebTarget> = self
            .entries
            .keys()
            .filter(|t| &t.cluster == cluster)
            .cloned()
            .collect();
        for target in targets {
            if let Some(entry) = self.entries.get_mut(&target) {
                entry.disconnected = !connected;
                entry.error = None;
            }
            self.sync(&target, cx);
        }
    }

    /// Remembers the local port once a forward listens (a stable origin next time), and drops
    /// forwards that went away.
    fn on_backend_changed(&mut self, cx: &mut Context<Self>) {
        let mut listening = Vec::new();
        for (target, entry) in &mut self.entries {
            let Some(id) = entry.forward else {
                continue;
            };
            match self.backend.info(id, cx) {
                Some(info) if info.local_port != 0 && !entry.remembered => {
                    entry.remembered = true;
                    listening.push((target.clone(), info.local_port));
                }
                Some(_) => {}
                None => entry.forward = None,
            }
        }
        for (target, local_port) in listening {
            if let Some(key) = PortKey::of(&target, cx) {
                store::update(cx, |state| {
                    state.update_port(&key, |m| m.local_port = Some(local_port))
                });
            }
        }
        cx.notify();
    }
}

fn close_tabs(entry: Entry, cx: &mut App) {
    for holder in entry.holders {
        if let Some(tab) = holder
            .tab
            .upgrade()
            .and_then(|t| t.downcast::<WebViewTab>().ok())
        {
            tab.update(cx, |tab, cx| tab.request_close(cx));
        }
    }
}

/// The real backend: forwards of `kubyl_portforward`'s manager.
#[derive(Default)]
struct ManagerBackend {
    ids: RefCell<HashMap<u64, ForwardId>>,
    next: RefCell<u64>,
}

impl ForwardBackend for ManagerBackend {
    fn start(&self, target: &WebTarget, https: bool, cx: &mut App) -> Result<u64, String> {
        let forward = start(target, https, cx)?;
        let mut next = self.next.borrow_mut();
        *next += 1;
        self.ids.borrow_mut().insert(*next, forward);
        Ok(*next)
    }

    fn stop(&self, id: u64, cx: &mut App) {
        if let Some(forward) = self.ids.borrow_mut().remove(&id) {
            PortForwardManager::global(cx).update(cx, |m, cx| m.stop(forward, cx));
        }
    }

    fn info(&self, id: u64, cx: &App) -> Option<ForwardInfo> {
        let forward = *self.ids.borrow().get(&id)?;
        PortForwardManager::global(cx).read(cx).info(forward)
    }
}

/// Starts the forward of `target` on loopback.
fn start(target: &WebTarget, https: bool, cx: &mut App) -> Result<ForwardId, String> {
    let Some(client) =
        ConnectionManager::try_global(cx).and_then(|m| m.read(cx).client(&target.cluster))
    else {
        return Err("The cluster isn't connected.".into());
    };
    // The port of last time keeps the page's origin (and its local storage) stable.
    let remembered = PortKey::of(target, cx)
        .map(|key| store::get(cx).port(&key))
        .and_then(|m| m.local_port);
    let local_port = remembered
        .filter(|&port| std::net::TcpListener::bind(("127.0.0.1", port)).is_ok())
        .unwrap_or(0);
    Ok(kubyl_portforward::start_forward(
        client,
        spec(target, https, local_port),
        cx,
    ))
}

/// The forward of a web view: loopback only, never saved, shown as `web view · …`.
pub fn spec(target: &WebTarget, https: bool, local_port: u16) -> ForwardSpec {
    let (kind, port) = match target.kind {
        TargetKind::Service => (
            ForwardKind::Service {
                service: target.name.clone(),
            },
            RemotePort::Service(Some(target.port)),
        ),
        TargetKind::Pod => (
            ForwardKind::Pod {
                pod: target.name.clone(),
            },
            RemotePort::Container(Some(target.port)),
        ),
    };
    let stop_target = target.clone();
    ForwardSpec {
        cluster: target.cluster.clone(),
        namespace: target.namespace.clone(),
        kind,
        port,
        // Loopback only: nothing is exposed on the network.
        bind_address: "127.0.0.1".into(),
        local_port,
        target: target.object(),
        remote_port: Some(target.port),
        http: true,
        https,
        open_browser: false,
        ephemeral: Some(Ephemeral {
            title: format!("web view · {target}"),
            buttons: Vec::new(),
            on_stop: Arc::new(move |cx| {
                let target = stop_target.clone();
                if let Some(forwards) = WebForwards::try_global(cx) {
                    forwards.update(cx, |this, cx| this.stopped_by_user(&target, cx));
                }
            }),
        }),
    }
}

/// A tab as [`WebForwards`] holds it.
pub fn holder(tab: &WeakEntity<WebViewTab>) -> AnyWeakEntity {
    tab.clone().into()
}

#[cfg(test)]
mod tests {
    use std::cell::Cell;

    use gpui::TestAppContext;

    use super::*;

    /// Counts starts and stops; every forward listens on port 40000 + id.
    #[derive(Default)]
    struct Fake {
        started: Cell<u64>,
        running: RefCell<Vec<u64>>,
        fail: Cell<bool>,
    }

    impl ForwardBackend for Fake {
        fn start(&self, _: &WebTarget, _: bool, _: &mut App) -> Result<u64, String> {
            if self.fail.get() {
                return Err("The cluster isn't connected.".into());
            }
            let id = self.started.get() + 1;
            self.started.set(id);
            self.running.borrow_mut().push(id);
            Ok(id)
        }

        fn stop(&self, id: u64, _: &mut App) {
            self.running.borrow_mut().retain(|r| *r != id);
        }

        fn info(&self, id: u64, _: &App) -> Option<ForwardInfo> {
            self.running.borrow().contains(&id).then(|| ForwardInfo {
                state: ForwardState::Listening,
                local_port: 40000 + id as u16,
                pod: Some("grafana-0".into()),
                remote_port: Some(3000),
                connections: 0,
                bytes_sent: 0,
                bytes_received: 0,
            })
        }
    }

    /// Stands in for a tab.
    struct Tab;

    fn target(cluster: &str) -> WebTarget {
        WebTarget {
            cluster: ClusterId::new(cluster),
            namespace: "monitoring".into(),
            kind: TargetKind::Service,
            name: "grafana".into(),
            port: 80,
        }
    }

    fn setup(cx: &mut TestAppContext) -> (Rc<Fake>, Entity<WebForwards>) {
        cx.update(|cx| {
            kubyl_core::init(cx);
            let fake = Rc::new(Fake::default());
            let forwards = WebForwards::install_with(fake.clone(), cx);
            (fake, forwards)
        })
    }

    fn tab(cx: &mut TestAppContext) -> Entity<Tab> {
        cx.update(|cx| cx.new(|_| Tab))
    }

    fn hold(
        forwards: &Entity<WebForwards>,
        tab: &Entity<Tab>,
        active: bool,
        cx: &mut TestAppContext,
    ) {
        let weak: AnyWeakEntity = tab.downgrade().into();
        forwards.update(cx, |f, cx| {
            f.hold(&target("a"), weak, active, Scheme::Http, cx)
        });
    }

    #[gpui::test]
    fn two_tabs_share_one_forward_until_the_last_closes(cx: &mut TestAppContext) {
        let (fake, forwards) = setup(cx);
        let (first, second) = (tab(cx), tab(cx));
        hold(&forwards, &first, true, cx);
        hold(&forwards, &second, true, cx);
        assert_eq!(fake.started.get(), 1, "one forward for both tabs");
        forwards.read_with(cx, |f, cx| {
            assert_eq!(f.tab_count(&target("a")), 2);
            assert!(matches!(
                f.status(&target("a"), cx),
                Some(ForwardStatus::Ready {
                    local_port: 40001,
                    ..
                })
            ));
        });

        let id = first.entity_id();
        forwards.update(cx, |f, cx| f.release(&target("a"), id, cx));
        assert_eq!(fake.running.borrow().len(), 1, "the other tab keeps it");

        let id = second.entity_id();
        forwards.update(cx, |f, cx| f.release(&target("a"), id, cx));
        assert!(
            fake.running.borrow().is_empty(),
            "stopped with the last tab"
        );
        forwards.read_with(cx, |f, cx| {
            assert_eq!(f.status(&target("a"), cx), None);
            assert_eq!(f.running().count(), 0);
        });
    }

    #[gpui::test]
    fn dropped_tabs_let_go_too(cx: &mut TestAppContext) {
        let (fake, forwards) = setup(cx);
        let first = tab(cx);
        hold(&forwards, &first, true, cx);
        drop(first);
        cx.run_until_parked();
        let second = tab(cx);
        hold(&forwards, &second, true, cx);
        forwards.read_with(cx, |f, _| assert_eq!(f.tab_count(&target("a")), 1));
        assert_eq!(fake.running.borrow().len(), 1);
    }

    #[gpui::test]
    fn idle_tabs_stop_the_forward_and_restart_it(cx: &mut TestAppContext) {
        let (fake, forwards) = setup(cx);
        let only = tab(cx);
        hold(&forwards, &only, true, cx);
        hold(&forwards, &only, false, cx);
        assert!(fake.running.borrow().is_empty());
        forwards.read_with(cx, |f, _| assert_eq!(f.tab_count(&target("a")), 1));
        hold(&forwards, &only, true, cx);
        assert_eq!(fake.started.get(), 2);
        assert_eq!(fake.running.borrow().len(), 1);
    }

    #[gpui::test]
    fn disconnects_stop_and_reconnects_restart(cx: &mut TestAppContext) {
        let (fake, forwards) = setup(cx);
        let only = tab(cx);
        hold(&forwards, &only, true, cx);
        forwards.update(cx, |f, cx| {
            f.cluster_changed(&ClusterId::new("a"), false, cx)
        });
        assert!(fake.running.borrow().is_empty());
        forwards.read_with(cx, |f, cx| {
            assert_eq!(
                f.status(&target("a"), cx),
                Some(ForwardStatus::Disconnected)
            )
        });
        // Another cluster's events don't matter.
        forwards.update(cx, |f, cx| {
            f.cluster_changed(&ClusterId::new("b"), true, cx)
        });
        assert!(fake.running.borrow().is_empty());
        forwards.update(cx, |f, cx| {
            f.cluster_changed(&ClusterId::new("a"), true, cx)
        });
        assert_eq!(fake.running.borrow().len(), 1);
    }

    #[gpui::test]
    fn failures_are_reported_and_retried(cx: &mut TestAppContext) {
        let (fake, forwards) = setup(cx);
        let only = tab(cx);
        fake.fail.set(true);
        hold(&forwards, &only, true, cx);
        forwards.read_with(cx, |f, cx| {
            assert_eq!(
                f.status(&target("a"), cx),
                Some(ForwardStatus::Failed("The cluster isn't connected.".into()))
            )
        });
        fake.fail.set(false);
        forwards.update(cx, |f, cx| {
            f.cluster_changed(&ClusterId::new("a"), true, cx)
        });
        assert_eq!(fake.running.borrow().len(), 1);
    }

    #[gpui::test]
    fn stop_and_close_ends_everything(cx: &mut TestAppContext) {
        let (fake, forwards) = setup(cx);
        let (first, second) = (tab(cx), tab(cx));
        hold(&forwards, &first, true, cx);
        hold(&forwards, &second, true, cx);
        forwards.update(cx, |f, cx| f.stop_and_close(&target("a"), cx));
        assert!(fake.running.borrow().is_empty());
        forwards.read_with(cx, |f, _| assert_eq!(f.tab_count(&target("a")), 0));
    }

    #[test]
    fn specs_bind_loopback_and_are_never_saved() {
        let spec = spec(&target("a"), true, 0);
        assert_eq!(spec.bind_address, "127.0.0.1");
        assert_eq!(spec.local_port, 0);
        assert!(spec.https);
        assert_eq!(
            spec.ephemeral.as_ref().map(|e| e.title.as_str()),
            Some("web view · svc/grafana:80")
        );
        assert_eq!(spec.port, RemotePort::Service(Some(80)));
    }
}
