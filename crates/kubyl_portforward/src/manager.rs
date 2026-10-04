//! The port-forward manager: the app's [`ForwardsCore`] (from `kubyl_portforward_core`), with
//! each forward shown in `kubyl_logs`'s Active Sessions panel with buttons to open it in a
//! browser, copy its address and save it.

use std::collections::HashMap;
use std::fmt;
use std::sync::Arc;

use gpui::{App, AppContext as _, ClipboardItem, Context, Entity, EventEmitter, Global};
use kubyl_core::forwards::ActiveForward;
use kubyl_core::host::{Hosts, hosted};
use kubyl_core::{ClusterId, Notification, NotificationCenter, ResourceRef, Tone};
use kubyl_logs::sessions::{SessionButton, SessionId, SessionKind, SessionRegistry};
pub use kubyl_portforward_core::manager::{ForwardInfo, ForwardState, human_bytes};
use kubyl_portforward_core::manager::{ForwardsCore, ForwardsEffect, Target};
use kubyl_ui::IconName;

use crate::favorites::{SavedForward, SavedForwards};
use crate::resolve::{ForwardKind, RemotePort};

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
        self.core_target().session_label()
    }

    /// `svc/ledger :5432`.
    pub fn target_label(&self) -> String {
        self.core_target().target_label()
    }

    fn core_target(&self) -> Target {
        Target {
            cluster: self.cluster.clone(),
            namespace: self.namespace.clone(),
            kind: self.kind.clone(),
            port: self.port.clone(),
            bind_address: self.bind_address.clone(),
            local_port: self.local_port,
            target: self.target.clone(),
            remote_port: self.remote_port,
            http: self.http,
            https: self.https,
            open_browser: self.open_browser,
            ephemeral: self.ephemeral.as_ref().map(|e| e.title.clone()),
        }
    }
}

/// What the UI keeps per forward next to the core's state.
struct Row {
    session_id: SessionId,
    saved: bool,
    ephemeral: Option<Ephemeral>,
}

#[derive(Default)]
pub struct PortForwardManager {
    core: ForwardsCore,
    rows: HashMap<u64, Row>,
}

impl EventEmitter<()> for PortForwardManager {}

impl Hosts<ForwardsCore> for PortForwardManager {
    fn service(&mut self) -> &mut ForwardsCore {
        &mut self.core
    }

    fn apply(&mut self, effect: ForwardsEffect, cx: &mut Context<Self>) {
        match effect {
            ForwardsEffect::Changed(id) => self.refresh_row(id, cx),
            ForwardsEffect::OpenUrl(url) => cx.open_url(&url),
        }
    }
}

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
        Self::start_with(client, spec, false, cx)
    }

    /// [`Self::start`]; `fallback_any`: a taken `local_port` falls back to a free one.
    pub fn start_with(
        client: kube::Client,
        spec: ForwardSpec,
        fallback_any: bool,
        cx: &mut App,
    ) -> ForwardId {
        let manager = Self::global(cx);
        let saved = spec.ephemeral.is_none()
            && SavedForwards::global(cx)
                .read(cx)
                .state()
                .contains(&saved_forward(&spec, cx));
        manager.update(cx, |this, cx| {
            let id = this.core.next_id();
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
            let target = spec.core_target();
            this.rows.insert(
                id,
                Row {
                    session_id,
                    saved,
                    ephemeral: spec.ephemeral,
                },
            );
            hosted(this, cx, |core, host| {
                core.start(id, client, target, fallback_any, host)
            });
            ForwardId(id)
        })
    }

    /// Stops a forward and removes its session row.
    pub fn stop(&mut self, id: ForwardId, cx: &mut Context<Self>) {
        if let Some(row) = self.rows.get(&id.0) {
            SessionRegistry::remove(cx, row.session_id);
        }
        self.remove(id.0, cx);
    }

    /// Drops a forward (its session row was removed already).
    fn remove(&mut self, id: u64, cx: &mut Context<Self>) -> Option<Row> {
        hosted(self, cx, |core, host| core.remove(id, host));
        self.rows.remove(&id)
    }

    /// The user stopped the forward from Active Sessions: drop it and tell a temporary
    /// forward's owner.
    fn stopped_by_user(&mut self, id: u64, cx: &mut Context<Self>) {
        let on_stop = self
            .remove(id, cx)
            .and_then(|row| row.ephemeral)
            .map(|e| e.on_stop);
        if let Some(on_stop) = on_stop {
            cx.defer(move |cx| on_stop(cx));
        }
    }

    /// The clusters that have forwards.
    pub fn clusters(&self) -> Vec<ClusterId> {
        self.core.clusters()
    }

    /// Stops the forwards whose cluster isn't connected any more (disconnected, reconnecting
    /// with a new client, or removed from the kubeconfig): they hold the old client and would
    /// keep a dead listener open. Saved auto-start forwards begin again when the cluster
    /// reconnects. A temporary forward's owner is told, like after a stop from Active Sessions.
    pub fn stop_disconnected(
        &mut self,
        connected: impl Fn(&ClusterId) -> bool,
        cx: &mut Context<Self>,
    ) {
        for id in self.core.disconnected(connected) {
            let (Some(row), Some(forward)) = (self.rows.get(&id), self.core.forward(id)) else {
                continue;
            };
            SessionRegistry::remove(cx, row.session_id);
            if row.ephemeral.is_none() {
                NotificationCenter::push(
                    cx,
                    Notification::info(format!(
                        "Port-forward {} stopped: the cluster disconnected.",
                        forward.target().target_label()
                    )),
                );
            }
            self.stopped_by_user(id, cx);
        }
    }

    /// A cluster entry's id changed without reconnecting: its forwards follow.
    pub fn rekey(&mut self, from: &ClusterId, to: &ClusterId) {
        self.core.rekey(from, to);
    }

    /// A forward's state, or `None` once it stopped.
    pub fn info(&self, id: ForwardId) -> Option<ForwardInfo> {
        self.core.info(id.0)
    }

    /// Stops the forward with a raw id from [`kubyl_core::forwards::ActiveForward::id`].
    pub fn stop_raw(&mut self, id: u64, cx: &mut Context<Self>) {
        self.stop(ForwardId(id), cx);
    }

    /// The running forwards as other crates see them
    /// ([`kubyl_core::forwards::ActiveForwards`]).
    pub fn active(&self) -> Vec<ActiveForward> {
        self.core.active()
    }

    /// Whether a forward to the same target and ports is running.
    pub fn is_running(&self, saved: &SavedForward, cx: &App) -> bool {
        self.core
            .forwards()
            .filter(|(_, f)| f.target().ephemeral.is_none())
            .any(|(_, f)| saved_target(f.target(), cx).same_target(saved))
    }

    /// Pushes title, status and buttons to the Active Sessions row.
    fn refresh_row(&mut self, id: u64, cx: &mut Context<Self>) {
        let (Some(forward), Some(row)) = (self.core.forward(id), self.rows.get(&id)) else {
            return;
        };
        let (status, tone) = forward.status();
        let session = row.session_id;
        let title = forward.title();
        let mut buttons = Vec::new();
        if forward.is_live() {
            if forward.target().http {
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
        let saved = row.saved;
        if let Some(ephemeral) = &row.ephemeral {
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
        let (Some(forward), Some(row)) = (self.core.forward(id), self.rows.get_mut(&id)) else {
            return;
        };
        if row.ephemeral.is_some() {
            return;
        }
        let saved = saved_target(forward.target(), cx);
        row.saved = !row.saved;
        let keep = row.saved;
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
        self.core.last_url()
    }
}

/// The saved-forward form of a spec (its context resolved through the connection manager).
pub fn saved_forward(spec: &ForwardSpec, cx: &App) -> SavedForward {
    saved_target(&spec.core_target(), cx)
}

fn saved_target(spec: &Target, cx: &App) -> SavedForward {
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
