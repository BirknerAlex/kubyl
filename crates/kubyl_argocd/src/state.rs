//! [`ArgoCd`]: the app-wide entity that knows, per cluster, which Argo CD CRDs are served
//! (from `ClusterCaps`, so it follows installs and uninstalls live), where Argo CD is installed
//! ([`crate::detect`]), and the API-mode session. The state machine is [`ArgoCore`] from
//! `kubyl_argocd_core`; this hands it snapshots of the connections, the settings and the
//! confirmed installs, and carries out what it asks for (the explorer's tree groups, state.json,
//! the browser).
//!
//! API mode only ever signs in to an install the user confirmed in the sign-in dialog
//! (namespace, Service and its UID, remembered per context in state.json). A stored token is
//! sent only to that Service; a re-created Service needs a new confirmation. The user's
//! Kubernetes credentials never reach Argo CD: through the service proxy they authenticate to the
//! API server (which strips them), through a forward they aren't sent at all.

use std::convert::Infallible;
use std::ops::Deref;

use gpui::{App, AppContext as _, Context, Entity, EventEmitter, Global, Subscription, Task};
use kube::discovery::ApiResource;
use kubyl_core::host::{Hosts, hosted};
use kubyl_core::{ArgoCdCaps, ClusterId, Gvr, Notification, NotificationCenter};
use kubyl_kube::{ConnectionEvent, ConnectionManager};
use kubyl_portforward::manager::PortForwardManager;
use kubyl_resources::store::{self, AppStores};
use kubyl_settings::{Settings, State};
use secrecy::SecretString;

pub use kubyl_argocd_core::state::*;

use crate::api::UserInfo;
use crate::detect::Install;
use crate::model::GROUP;
use crate::settings::{ArgoSettings, ArgoState};

/// Argo CD state of every cluster. One per app: [`ArgoCd::global`]. Reads go to the
/// [`ArgoCore`]; observe the entity to re-render.
pub struct ArgoCd {
    core: ArgoCore,
    _subscriptions: Vec<Subscription>,
}

impl Deref for ArgoCd {
    type Target = ArgoCore;

    fn deref(&self) -> &ArgoCore {
        &self.core
    }
}

impl EventEmitter<Infallible> for ArgoCd {}

impl Hosts<ArgoCore> for ArgoCd {
    fn service(&mut self) -> &mut ArgoCore {
        &mut self.core
    }

    fn apply(&mut self, effect: ArgoEffect, cx: &mut Context<Self>) {
        match effect {
            ArgoEffect::TreeGroupsChanged => kubyl_explorer::catalog::tree_groups_changed(cx),
            ArgoEffect::SetState(state) => State::update::<ArgoState>(cx, |s| *s = state),
            ArgoEffect::OpenUrl(url) => {
                if let Err(err) = open::that_detached(&url) {
                    tracing::warn!("couldn't open the browser: {err}");
                }
            }
            ArgoEffect::DetectAgain(cluster) => {
                self.sync_inputs(&cluster, cx);
                let hints = controller_namespaces(&cluster, &AppStores(cx));
                hosted(self, cx, |core, host| {
                    core.detect_again(&cluster, hints, host)
                });
            }
        }
    }
}

struct GlobalArgoCd(Entity<ArgoCd>);

impl Global for GlobalArgoCd {}

impl ArgoCd {
    pub fn install(cx: &mut App) -> Entity<Self> {
        let entity = cx.new(|cx| {
            let mut subscriptions = Vec::new();
            if let Some(manager) = ConnectionManager::try_global(cx) {
                subscriptions.push(cx.subscribe(
                    &manager,
                    |this: &mut ArgoCd, _, event: &ConnectionEvent, cx| match event {
                        ConnectionEvent::DiscoveryChanged(id)
                        | ConnectionEvent::StateChanged(id) => this.sync_cluster(id, cx),
                        ConnectionEvent::ContextsChanged => {
                            for id in this.core.clusters() {
                                this.sync_cluster(&id, cx);
                            }
                        }
                        _ => {}
                    },
                ));
            }
            let weak = cx.weak_entity();
            subscriptions.push(Settings::observe::<ArgoSettings>(
                cx,
                move |settings, cx| {
                    let settings = settings.clone();
                    weak.update(cx, |this, _| this.core.set_settings(settings))
                        .ok();
                },
            ));
            let mut core = ArgoCore::new(PortForwardManager::app_reach(cx));
            core.set_settings(Settings::get::<ArgoSettings>(cx).clone());
            core.set_state(State::get::<ArgoState>(cx));
            Self {
                core,
                _subscriptions: subscriptions,
            }
        });
        cx.set_global(GlobalArgoCd(entity.clone()));
        entity
    }

    pub fn global(cx: &App) -> Entity<Self> {
        cx.global::<GlobalArgoCd>().0.clone()
    }

    pub fn try_global(cx: &App) -> Option<Entity<Self>> {
        cx.try_global::<GlobalArgoCd>().map(|g| g.0.clone())
    }

    /// Which Argo CD CRDs `cluster` serves (live, from discovery).
    pub fn caps(cluster: &ClusterId, cx: &App) -> ArgoCdCaps {
        ConnectionManager::try_global(cx)
            .map(|m| m.read(cx).caps(cluster).argocd)
            .unwrap_or_default()
    }

    /// Hands the core what it reads from the app: the settings, the confirmed installs and the
    /// connection of `cluster`.
    fn sync_inputs(&mut self, cluster: &ClusterId, cx: &App) {
        self.core
            .set_settings(Settings::get::<ArgoSettings>(cx).clone());
        self.core.set_state(State::get::<ArgoState>(cx));
        self.core.set_link(cluster, link(cluster, cx));
    }

    /// Follows a cluster's caps and connection: starts detection when CRDs appear, drops
    /// everything when they go or the cluster disconnects.
    fn sync_cluster(&mut self, cluster: &ClusterId, cx: &mut Context<Self>) {
        let Some(manager) = ConnectionManager::try_global(cx) else {
            return;
        };
        let (caps, connected) = {
            let m = manager.read(cx);
            (m.caps(cluster).argocd, m.state(cluster).is_connected())
        };
        self.sync_inputs(cluster, cx);
        let hints = controller_namespaces(cluster, &AppStores(cx));
        let link = link(cluster, cx);
        hosted(self, cx, |core, host| {
            core.sync_cluster(cluster, caps, connected, link, hints, host)
        });
    }

    /// Detects again (the user asked, or the install changed).
    pub fn redetect(&mut self, cluster: &ClusterId, cx: &mut Context<Self>) {
        self.sync_inputs(cluster, cx);
        let hints = controller_namespaces(cluster, &AppStores(cx));
        hosted(self, cx, |core, host| core.redetect(cluster, hints, host));
    }

    /// The detected install the user confirmed before, if it's still the same Service.
    pub fn trusted_install(&self, cluster: &ClusterId, _cx: &App) -> Option<Install> {
        self.core.trusted_install(cluster)
    }

    /// Why the confirmed install can't be used any more (a re-created Service).
    pub fn trust_problem(&self, cluster: &ClusterId, _cx: &App) -> Option<String> {
        self.core.trust_problem(cluster)
    }

    /// Connects API mode to the confirmed install with the stored token (no-op without one).
    pub fn connect(&mut self, cluster: &ClusterId, cx: &mut Context<Self>) {
        self.sync_inputs(cluster, cx);
        hosted(self, cx, |core, host| core.connect(cluster, host));
    }

    /// The session token for a web view of `namespace/service`: only for the argocd-server
    /// Service of the confirmed install, and only while API mode is signed in there.
    pub fn web_session_token(
        &self,
        cluster: &ClusterId,
        namespace: &str,
        service: &str,
        _cx: &App,
    ) -> Option<SecretString> {
        self.core.web_session_token(cluster, namespace, service)
    }

    /// Signs in to `install` (the user confirmed it in the dialog) and remembers it.
    pub fn sign_in(
        &mut self,
        cluster: &ClusterId,
        install: Install,
        credentials: Credentials,
        cx: &mut Context<Self>,
    ) -> Task<Result<UserInfo, String>> {
        self.sync_inputs(cluster, cx);
        let answer = hosted(self, cx, |core, host| {
            core.sign_in(cluster, install, credentials, host)
        });
        cx.spawn(async move |_, _| {
            answer
                .await
                .unwrap_or_else(|_| Err("The sign-in was cancelled.".into()))
        })
    }

    /// Signs out: ends the session on the server (the token is revoked, so a web view's copy
    /// of it stops working too), forgets the token, stops the forward; Kubernetes mode stays.
    pub fn sign_out(&mut self, cluster: &ClusterId, forget_install: bool, cx: &mut Context<Self>) {
        self.sync_inputs(cluster, cx);
        hosted(self, cx, |core, host| {
            core.sign_out(cluster, forget_install, host)
        });
    }

    /// An API call said the token is no longer valid. An SSO session renews itself, a password
    /// session signs in again (once a minute at most); otherwise the user signs in again.
    pub fn unauthorized(&mut self, cluster: &ClusterId, cx: &mut Context<Self>) {
        self.sync_inputs(cluster, cx);
        hosted(self, cx, |core, host| core.unauthorized(cluster, host));
    }
}

/// The connection of `cluster` as the core sees it.
fn link(cluster: &ClusterId, cx: &App) -> Link {
    let Some(manager) = ConnectionManager::try_global(cx) else {
        return Link::default();
    };
    let manager = manager.read(cx);
    Link {
        client: manager.client(cluster),
        context: manager.context(cluster).cloned(),
    }
}

/// The served resource for an Argo CD kind (`applications`, `applicationsets`, `appprojects`):
/// its GVR (preferred version) and kube `ApiResource`.
pub fn resource(cluster: &ClusterId, plural: &str, cx: &App) -> Option<(Gvr, ApiResource)> {
    let discovery = ConnectionManager::try_global(cx)?
        .read(cx)
        .discovery(cluster)?;
    let info = store::find_resource(&discovery.resources, &Gvr::new(GROUP, "", plural))?;
    Some((info.gvr.clone(), store::api_resource(info)))
}

/// Who Kubernetes-mode operations name as their initiator: the Kubernetes user, when known.
pub fn username(cluster: &ClusterId, cx: &App) -> String {
    ConnectionManager::try_global(cx)
        .and_then(|m| m.read(cx).cluster(cluster)?.info.as_ref()?.user.clone())
        .filter(|u| !u.is_empty())
        .unwrap_or_else(|| "kubyl".into())
}

/// Shows an action's failure as a toast (Argo CD's 403s say so plainly).
pub fn notify_error(what: &str, error: impl std::fmt::Display, cx: &mut App) {
    NotificationCenter::push(cx, Notification::error(format!("{what}: {error}")));
}
