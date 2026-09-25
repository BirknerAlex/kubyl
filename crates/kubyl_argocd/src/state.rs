//! [`ArgoCd`]: the app-wide entity that knows, per cluster, which Argo CD CRDs are served
//! (from `ClusterCaps`, so it follows installs and uninstalls live), where Argo CD is installed
//! ([`crate::detect`]), and the API-mode session.
//!
//! API mode only ever signs in to an install the user confirmed in the sign-in dialog
//! (namespace, Service and its UID, remembered per context in state.json). A stored token is
//! sent only to that Service; a re-created Service needs a new confirmation. The user's
//! Kubernetes credentials never reach Argo CD: through the service proxy they authenticate to the
//! API server (which strips them), through a forward they aren't sent at all.
//!
//! SSO sessions ([`crate::sso`]) keep their refresh token in the keychain next to the session
//! token and renew themselves when the token expires or the server rejects it.

use std::collections::HashMap;
use std::sync::Arc;
use std::time::{Duration, Instant};

use gpui::{App, AppContext as _, AsyncApp, Context, Entity, Global, Subscription, Task};
use kube::discovery::ApiResource;
use kubyl_core::{
    ArgoCdCaps, ClusterId, Gvr, Notification, NotificationCenter, ResourceRef, spawn_kube,
};
use kubyl_kube::{ConnectionEvent, ConnectionManager};
use kubyl_portforward::manager::{
    Ephemeral, ForwardId, ForwardInfo, ForwardSpec, ForwardState, PortForwardManager,
};
use kubyl_portforward::resolve::{ForwardKind, RemotePort};
use kubyl_resources::{ResourceStores, store};
use secrecy::SecretString;

use crate::api::{ApiError, ArgoApi, Transport, UserInfo};
use crate::detect::{self, Install};
use crate::model::GROUP;
use crate::settings::{self, ApiTransport, ContextKey};
use crate::sso::{self, SsoConfig, SsoTokens};

/// How long a temporary forward may take to listen.
const FORWARD_TIMEOUT: Duration = Duration::from_secs(20);
/// While Argo CD is being installed (CRDs first, workloads later), detection finds nothing or
/// no version. It runs again: 3 s doubling, 5 times.
const DETECT_RETRY: Duration = Duration::from_secs(3);
const DETECT_RETRIES: u32 = 5;

/// Where detection of a cluster's installs stands.
#[derive(Clone, Debug, Default, PartialEq)]
pub enum Detection {
    #[default]
    Idle,
    Running,
    Done(Arc<Vec<Install>>),
    Failed(String),
}

/// The API-mode session of a cluster.
#[derive(Clone, Debug, Default, PartialEq)]
pub enum ApiState {
    /// Kubernetes mode only.
    #[default]
    Off,
    Connecting,
    /// A confirmed install, but no valid token (never signed in, expired or revoked).
    SignInRequired(Option<String>),
    Connected {
        user: String,
        version: String,
        /// `the API server's service proxy` / `a temporary port-forward`.
        via: &'static str,
    },
    Failed(String),
}

impl ApiState {
    pub fn is_connected(&self) -> bool {
        matches!(self, ApiState::Connected { .. })
    }
}

/// How the user signs in.
pub enum Credentials {
    Password {
        username: String,
        password: SecretString,
    },
    /// An API token (from `argocd account generate-token`) or an SSO session token.
    Token(SecretString),
    /// Sign in through the browser (SSO). The sign-in URL is sent to the channel too.
    Sso(Option<futures::channel::mpsc::UnboundedSender<String>>),
}

struct ApiSession {
    state: ApiState,
    install: Install,
    /// The transport (with the token once connected).
    api: Option<ArgoApi>,
    forward: Option<ForwardId>,
    /// An SSO session with a refresh token: a rejected token renews it.
    renewable: bool,
    /// When the session last reconnected after a rejected token.
    renewed_at: Option<Instant>,
    _task: Option<Task<()>>,
}

impl ApiSession {
    fn new(install: Install) -> Self {
        Self {
            state: ApiState::Connecting,
            install,
            api: None,
            forward: None,
            renewable: false,
            renewed_at: None,
            _task: None,
        }
    }
}

#[derive(Default)]
struct ClusterArgo {
    caps: ArgoCdCaps,
    detection: Detection,
    _detect_task: Option<Task<()>>,
    detect_retries: u32,
    _retry_task: Option<Task<()>>,
    api: Option<ApiSession>,
    /// A sign-in in progress. Owned here, not by the dialog: an SSO sign-in waits for the
    /// browser and must survive the dialog closing.
    _sign_in: Option<Task<()>>,
}

/// Argo CD state of every cluster. One per app: [`ArgoCd::global`].
pub struct ArgoCd {
    clusters: HashMap<ClusterId, ClusterArgo>,
    _subscriptions: Vec<Subscription>,
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
                            let ids: Vec<ClusterId> = this.clusters.keys().cloned().collect();
                            for id in ids {
                                this.sync_cluster(&id, cx);
                            }
                        }
                        _ => {}
                    },
                ));
            }
            Self {
                clusters: HashMap::new(),
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
        let entry = self.clusters.entry(cluster.clone()).or_default();
        let changed = entry.caps != caps;
        entry.caps = caps;
        if !connected || !caps.any() {
            let had = entry.api.is_some() || entry.detection != Detection::Idle;
            if let Some(session) = entry.api.take() {
                stop_forward(session.forward, cx);
            }
            entry.detection = Detection::Idle;
            entry._detect_task = None;
            entry._retry_task = None;
            entry._sign_in = None;
            if had || changed {
                kubyl_explorer::catalog::tree_groups_changed(cx);
                cx.notify();
            }
            return;
        }
        if changed {
            // CRDs came or went: look again.
            entry.detection = Detection::Idle;
            entry.detect_retries = 0;
            kubyl_explorer::catalog::tree_groups_changed(cx);
            cx.notify();
        }
        if entry.detection == Detection::Idle {
            self.detect(cluster, cx);
        }
    }

    // ----- Detection -----

    /// Where detection of `cluster` stands.
    pub fn detection(&self, cluster: &ClusterId) -> Detection {
        self.clusters
            .get(cluster)
            .map(|c| c.detection.clone())
            .unwrap_or_default()
    }

    /// The installs found on `cluster` (empty while detecting).
    pub fn installs(&self, cluster: &ClusterId) -> Arc<Vec<Install>> {
        match self.clusters.get(cluster).map(|c| &c.detection) {
            Some(Detection::Done(installs)) => installs.clone(),
            _ => Arc::new(Vec::new()),
        }
    }

    /// The install that manages Applications in `namespace` (the first one when unsure).
    pub fn install_for(&self, cluster: &ClusterId, namespace: Option<&str>) -> Option<Install> {
        let installs = self.installs(cluster);
        namespace
            .and_then(|ns| installs.iter().find(|i| i.manages_namespace(ns)))
            .or_else(|| installs.first())
            .cloned()
    }

    /// Detects again (the user asked, or the install changed).
    pub fn redetect(&mut self, cluster: &ClusterId, cx: &mut Context<Self>) {
        if let Some(entry) = self.clusters.get_mut(cluster) {
            entry.detection = Detection::Idle;
            entry.detect_retries = 0;
        }
        self.detect(cluster, cx);
    }

    fn detect(&mut self, cluster: &ClusterId, cx: &mut Context<Self>) {
        let Some(client) =
            ConnectionManager::try_global(cx).and_then(|m| m.read(cx).client(cluster))
        else {
            return;
        };
        let hints = controller_namespaces(cluster, cx);
        let Some(entry) = self.clusters.get_mut(cluster) else {
            return;
        };
        // A retry keeps showing what the last run found.
        if !matches!(entry.detection, Detection::Done(_)) {
            entry.detection = Detection::Running;
        }
        entry._retry_task = None;
        let task = spawn_kube(cx, detect::detect(client, hints));
        let id = cluster.clone();
        entry._detect_task = Some(cx.spawn(async move |this, cx| {
            let result = task.await;
            this.update(cx, |this, cx| {
                let Some(entry) = this.clusters.get_mut(&id) else {
                    return;
                };
                let unfinished = match &result {
                    Ok(installs) => installs.iter().all(|i| i.version.is_none()),
                    Err(_) => true,
                };
                entry.detection = match result {
                    Ok(installs) => Detection::Done(Arc::new(installs)),
                    Err(err) => {
                        tracing::info!(cluster = %id, "Argo CD detection failed: {err}");
                        Detection::Failed(err)
                    }
                };
                if unfinished && entry.detect_retries < DETECT_RETRIES {
                    let delay = DETECT_RETRY * 2u32.pow(entry.detect_retries);
                    entry.detect_retries += 1;
                    let retry = id.clone();
                    entry._retry_task = Some(cx.spawn(async move |this, cx| {
                        cx.background_executor().timer(delay).await;
                        this.update(cx, |this, cx| this.detect(&retry, cx)).ok();
                    }));
                }
                kubyl_explorer::catalog::tree_groups_changed(cx);
                cx.notify();
                this.auto_connect(&id, cx);
            })
            .ok();
        }));
        cx.notify();
    }

    // ----- API mode -----

    /// The API-mode state of `cluster`.
    pub fn api_state(&self, cluster: &ClusterId) -> ApiState {
        self.clusters
            .get(cluster)
            .and_then(|c| c.api.as_ref())
            .map(|s| s.state.clone())
            .unwrap_or_default()
    }

    /// A signed-in client, when API mode is connected.
    pub fn api(&self, cluster: &ClusterId) -> Option<ArgoApi> {
        let session = self.clusters.get(cluster)?.api.as_ref()?;
        session
            .state
            .is_connected()
            .then(|| session.api.clone())
            .flatten()
    }

    /// The install API mode talks to.
    pub fn api_install(&self, cluster: &ClusterId) -> Option<Install> {
        Some(self.clusters.get(cluster)?.api.as_ref()?.install.clone())
    }

    /// The detected install the user confirmed before, if it's still the same Service.
    pub fn trusted_install(&self, cluster: &ClusterId, cx: &App) -> Option<Install> {
        let trusted = settings::trusted(cluster, cx)?;
        self.installs(cluster)
            .iter()
            .find(|i| trusted.matches(i))
            .cloned()
    }

    /// Why the confirmed install can't be used any more (a re-created Service).
    pub fn trust_problem(&self, cluster: &ClusterId, cx: &App) -> Option<String> {
        let trusted = settings::trusted(cluster, cx)?;
        let installs = self.installs(cluster);
        if installs.is_empty() || installs.iter().any(|i| trusted.matches(i)) {
            return None;
        }
        let same_name = installs.iter().any(|i| {
            i.namespace == trusted.namespace
                && i.server.as_ref().is_some_and(|s| s.name == trusted.service)
        });
        Some(if same_name {
            format!(
                "The {}/{} Service was re-created since you confirmed it. Check that it's still your Argo CD before signing in.",
                trusted.namespace, trusted.service
            )
        } else {
            format!(
                "The Argo CD you confirmed ({}/{}) isn't there any more.",
                trusted.namespace, trusted.service
            )
        })
    }

    fn auto_connect(&mut self, cluster: &ClusterId, cx: &mut Context<Self>) {
        let already = self.clusters.get(cluster).is_some_and(|c| c.api.is_some());
        if already || !settings::get(cx).auto_connect {
            return;
        }
        if self.trusted_install(cluster, cx).is_some() {
            self.connect(cluster, cx);
        }
    }

    /// Connects API mode to the confirmed install with the stored token (no-op without one).
    pub fn connect(&mut self, cluster: &ClusterId, cx: &mut Context<Self>) {
        let Some(install) = self.trusted_install(cluster, cx) else {
            return;
        };
        let Some(key) = ContextKey::of(cluster, cx) else {
            return;
        };
        let Some(entry) = self.clusters.get_mut(cluster) else {
            return;
        };
        if let Some(old) = entry.api.take() {
            stop_forward(old.forward, cx);
        }
        let mut session = ApiSession::new(install.clone());
        let id = cluster.clone();
        session._task = Some(cx.spawn(async move |this, cx| {
            let result = async {
                let (api, forward) = open_transport(&id, &install, cx).await?;
                this.update(cx, |this, _| {
                    if let Some(session) = this.session_mut(&id) {
                        session.forward = forward;
                        session.api = Some(api.clone());
                    }
                })
                .ok();
                let version = run(cx, {
                    let api = api.clone();
                    async move { api.version().await }
                })
                .await
                .map_err(|e| e.to_string())?;
                let service = install
                    .server
                    .as_ref()
                    .map(|s| s.name.clone())
                    .unwrap_or_default();
                let token_key = settings::token_key(&key, &install.namespace, &service);
                let mut token = read_token(cx, token_key.clone()).await;
                let refresh = read_token(cx, refresh_key(&token_key)).await;
                this.update(cx, |this, _| {
                    if let Some(session) = this.session_mut(&id) {
                        session.renewable = refresh.is_some();
                    }
                })
                .ok();
                // An SSO session renews itself before it expires, and once if it's rejected.
                let mut renewed = false;
                if let Some(refresh) = &refresh
                    && token.as_ref().is_none_or(sso::expiring)
                {
                    renewed = true;
                    match renew(cx, &api, &token_key, refresh).await {
                        Ok(fresh) => token = Some(fresh),
                        Err(err) => tracing::info!("Argo CD session renewal failed: {err}"),
                    }
                }
                let Some(mut token) = token else {
                    return Ok::<_, String>(ApiState::SignInRequired(None));
                };
                loop {
                    let signed = api.with_token(token.clone());
                    let check = run(cx, {
                        let signed = signed.clone();
                        async move { signed.user_info().await }
                    })
                    .await;
                    match check {
                        Ok(info) => {
                            let via = api.transport().describe();
                            this.update(cx, |this, _| {
                                if let Some(session) = this.session_mut(&id) {
                                    session.api = Some(signed);
                                }
                            })
                            .ok();
                            return Ok(ApiState::Connected {
                                user: info.username,
                                version,
                                via,
                            });
                        }
                        Err(ApiError::Unauthorized) if !renewed && refresh.is_some() => {
                            renewed = true;
                            let refresh = refresh.as_ref().expect("checked");
                            match renew(cx, &api, &token_key, refresh).await {
                                Ok(fresh) => token = fresh,
                                Err(err) => {
                                    return Ok(ApiState::SignInRequired(Some(format!(
                                        "The SSO session couldn't be renewed ({err}). Sign in again."
                                    ))));
                                }
                            }
                        }
                        Err(ApiError::Unauthorized) => {
                            return Ok(ApiState::SignInRequired(Some(
                                "The session expired. Sign in again.".into(),
                            )));
                        }
                        Err(err) => return Err(err.to_string()),
                    }
                }
            }
            .await;
            this.update(cx, |this, cx| {
                if let Some(session) = this.session_mut(&id) {
                    session.state = match result {
                        Ok(state) => state,
                        Err(err) => ApiState::Failed(err),
                    };
                }
                cx.notify();
            })
            .ok();
        }));
        entry.api = Some(session);
        cx.notify();
    }

    /// The session token for a web view of `namespace/service`: only for the argocd-server
    /// Service of the confirmed install, and only while API mode is signed in there.
    pub fn web_session_token(
        &self,
        cluster: &ClusterId,
        namespace: &str,
        service: &str,
        cx: &App,
    ) -> Option<SecretString> {
        let session = self.clusters.get(cluster)?.api.as_ref()?;
        if !session.state.is_connected() {
            return None;
        }
        let trusted = self.trusted_install(cluster, cx)?;
        let server = trusted.server.as_ref()?;
        let ours = trusted.namespace == namespace
            && server.name == service
            && session.install.namespace == namespace;
        if !ours {
            return None;
        }
        session.api.as_ref()?.token().cloned()
    }

    fn session_mut(&mut self, cluster: &ClusterId) -> Option<&mut ApiSession> {
        self.clusters.get_mut(cluster)?.api.as_mut()
    }

    /// Signs in to `install` (the user confirmed it in the dialog) and remembers it.
    pub fn sign_in(
        &mut self,
        cluster: &ClusterId,
        install: Install,
        credentials: Credentials,
        cx: &mut Context<Self>,
    ) -> Task<Result<UserInfo, String>> {
        let Some(key) = ContextKey::of(cluster, cx) else {
            return Task::ready(Err("unknown context".into()));
        };
        let Some(service) = install.server.as_ref().map(|s| s.name.clone()) else {
            return Task::ready(Err(format!(
                "No argocd-server Service found in {}",
                install.namespace
            )));
        };
        let entry = self.clusters.entry(cluster.clone()).or_default();
        // Reuse a transport to the same install; otherwise start over.
        let reuse = entry
            .api
            .as_ref()
            .filter(|s| s.install == install)
            .and_then(|s| s.api.clone().map(|api| (api, s.forward)));
        if reuse.is_none()
            && let Some(old) = entry.api.take()
        {
            stop_forward(old.forward, cx);
        }
        if entry.api.is_none() {
            entry.api = Some(ApiSession::new(install.clone()));
        } else if let Some(session) = entry.api.as_mut() {
            session.state = ApiState::Connecting;
        }
        cx.notify();
        let id = cluster.clone();
        let (done, result) = futures::channel::oneshot::channel();
        let work = cx.spawn(async move |this, cx| {
            let result = async {
                let (api, forward) = match reuse {
                    Some(reused) => reused,
                    None => open_transport(&id, &install, cx).await?,
                };
                this.update(cx, |this, _| {
                    if let Some(session) = this.session_mut(&id) {
                        session.forward = forward;
                        session.api = Some(api.clone());
                    }
                })
                .ok();
                let (token, session) = match credentials {
                    Credentials::Token(token) => (token, None),
                    Credentials::Password { username, password } => (
                        run(cx, {
                            let api = api.clone();
                            async move { api.login(&username, &password).await }
                        })
                        .await
                        .map_err(|e| e.to_string())?,
                        None,
                    ),
                    Credentials::Sso(urls) => {
                        let tokens = run(cx, {
                            let api = api.clone();
                            async move {
                                let settings = api.settings().await.map_err(|e| e.to_string())?;
                                let config = SsoConfig::from_settings(&settings)?;
                                sso::sign_in(&config, move |url| {
                                    if let Err(err) = open::that_detached(&url) {
                                        tracing::warn!("couldn't open the browser: {err}");
                                    }
                                    if let Some(urls) = urls {
                                        urls.unbounded_send(url).ok();
                                    }
                                })
                                .await
                            }
                        })
                        .await?;
                        (tokens.id_token.clone(), Some(tokens))
                    }
                };
                let signed = api.with_token(token.clone());
                let (info, version) = run(cx, {
                    let signed = signed.clone();
                    async move {
                        let info = signed.user_info().await?;
                        let version = signed.version().await.unwrap_or_default();
                        Ok::<_, ApiError>((info, version))
                    }
                })
                .await
                .map_err(|e| e.to_string())?;
                let token_key = settings::token_key(&key, &install.namespace, &service);
                let renewable = session.as_ref().is_some_and(|s| s.refresh_token.is_some());
                match &session {
                    Some(session) => store_session(cx, &token_key, session).await?,
                    None => {
                        write_token(cx, token_key.clone(), token).await?;
                        // A refresh token of an earlier SSO session would renew the wrong one.
                        delete_token(cx, refresh_key(&token_key)).await;
                    }
                }
                let via = api.transport().describe();
                this.update(cx, |this, cx| {
                    settings::trust(&id, &install, Some(info.username.clone()), cx);
                    if let Some(session) = this.session_mut(&id) {
                        session.renewable = renewable;
                        session.api = Some(signed);
                        session.state = ApiState::Connected {
                            user: info.username.clone(),
                            version,
                            via,
                        };
                    }
                    cx.notify();
                })
                .ok();
                Ok::<_, String>(info)
            }
            .await;
            this.update(cx, |this, cx| {
                match &result {
                    Ok(info) => NotificationCenter::push(
                        cx,
                        Notification::info(format!("Signed in to Argo CD as {}", info.username)),
                    ),
                    Err(err) => {
                        if let Some(session) = this.session_mut(&id) {
                            session.state = ApiState::SignInRequired(Some(err.clone()));
                        }
                    }
                }
                cx.notify();
            })
            .ok();
            done.send(result).ok();
        });
        // Replaces (cancels) a sign-in still waiting for the browser.
        if let Some(entry) = self.clusters.get_mut(cluster) {
            entry._sign_in = Some(work);
        }
        cx.spawn(async move |_, _| {
            result
                .await
                .unwrap_or_else(|_| Err("The sign-in was cancelled.".into()))
        })
    }

    /// Signs out: forgets the token, stops the forward; Kubernetes mode stays.
    pub fn sign_out(&mut self, cluster: &ClusterId, forget_install: bool, cx: &mut Context<Self>) {
        let session = self.clusters.get_mut(cluster).and_then(|c| c.api.take());
        if let Some(session) = &session
            && let (Some(key), Some(server)) =
                (ContextKey::of(cluster, cx), &session.install.server)
        {
            let token_key = settings::token_key(&key, &session.install.namespace, &server.name);
            spawn_kube(cx, async move {
                tokio::task::spawn_blocking(move || {
                    kubyl_kube::auth::store::delete(&refresh_key(&token_key)).ok();
                    kubyl_kube::auth::store::delete(&token_key)
                })
                .await
                .ok();
            })
            .detach();
        }
        if let Some(session) = session {
            stop_forward(session.forward, cx);
        }
        if forget_install {
            settings::forget(cluster, cx);
        }
        cx.notify();
    }

    /// An API call said the token is no longer valid. An SSO session renews itself (once a
    /// minute at most); otherwise the user signs in again.
    pub fn unauthorized(&mut self, cluster: &ClusterId, cx: &mut Context<Self>) {
        let renew = self.session_mut(cluster).is_some_and(|s| {
            s.renewable
                && s.renewed_at
                    .is_none_or(|t| t.elapsed() > Duration::from_secs(60))
        });
        if renew {
            self.connect(cluster, cx);
            if let Some(session) = self.session_mut(cluster) {
                session.renewed_at = Some(Instant::now());
            }
            return;
        }
        if let Some(session) = self.session_mut(cluster) {
            session.state = ApiState::SignInRequired(Some(
                "The Argo CD session expired. Sign in again.".into(),
            ));
            cx.notify();
        }
    }

    /// The user stopped API mode's forward in Active Sessions.
    fn forward_stopped(&mut self, cluster: &ClusterId, cx: &mut Context<Self>) {
        if let Some(session) = self.session_mut(cluster) {
            session.forward = None;
            session.api = None;
            session.state =
                ApiState::Failed("The port-forward to argocd-server was stopped.".into());
            cx.notify();
        }
    }
}

/// Stops API mode's forward (it exists only if the port-forward manager started it).
fn stop_forward(forward: Option<ForwardId>, cx: &mut App) {
    if let Some(id) = forward {
        PortForwardManager::global(cx).update(cx, |m, cx| m.stop(id, cx));
    }
}

/// Runs `future` on the Tokio runtime from an async GPUI context.
async fn run<T: Send + 'static>(
    cx: &mut AsyncApp,
    future: impl std::future::Future<Output = T> + Send + 'static,
) -> T {
    let task = cx.update(|cx| spawn_kube(cx, future));
    task.await
}

async fn read_token(cx: &mut AsyncApp, key: String) -> Option<SecretString> {
    run(cx, async move {
        tokio::task::spawn_blocking(move || kubyl_kube::auth::store::get(&key))
            .await
            .ok()
            .and_then(|r| r.inspect_err(|e| tracing::warn!("keychain: {e}")).ok())
            .flatten()
    })
    .await
}

/// Where an SSO session's refresh token is kept, next to its session token.
fn refresh_key(token_key: &str) -> String {
    format!("{token_key}/refresh")
}

async fn delete_token(cx: &mut AsyncApp, key: String) {
    run(cx, async move {
        tokio::task::spawn_blocking(move || kubyl_kube::auth::store::delete(&key))
            .await
            .ok();
    })
    .await;
}

/// Stores an SSO session: the refresh token, and the session token when the keychain takes it
/// (Windows Credential Manager holds 2.5 KB; without it the next start renews the session).
async fn store_session(
    cx: &mut AsyncApp,
    token_key: &str,
    tokens: &SsoTokens,
) -> Result<(), String> {
    let Some(refresh) = &tokens.refresh_token else {
        delete_token(cx, refresh_key(token_key)).await;
        return write_token(cx, token_key.to_string(), tokens.id_token.clone()).await;
    };
    write_token(cx, refresh_key(token_key), refresh.clone()).await?;
    if let Err(err) = write_token(cx, token_key.to_string(), tokens.id_token.clone()).await {
        tracing::info!("Argo CD session token not stored: {err}");
    }
    Ok(())
}

/// Renews an SSO session with its refresh token (settings from the confirmed server say where)
/// and stores the new tokens.
async fn renew(
    cx: &mut AsyncApp,
    api: &ArgoApi,
    token_key: &str,
    refresh: &SecretString,
) -> Result<SecretString, String> {
    let tokens = run(cx, {
        let api = api.clone();
        let refresh = refresh.clone();
        async move {
            let settings = api.settings().await.map_err(|e| e.to_string())?;
            let config = SsoConfig::from_settings(&settings)?;
            sso::refresh(&config, &refresh).await
        }
    })
    .await?;
    store_session(cx, token_key, &tokens).await?;
    Ok(tokens.id_token)
}

async fn write_token(cx: &mut AsyncApp, key: String, token: SecretString) -> Result<(), String> {
    run(cx, async move {
        tokio::task::spawn_blocking(move || kubyl_kube::auth::store::set(&key, &token))
            .await
            .map_err(|e| e.to_string())?
    })
    .await
    .map_err(|e| {
        format!(
            "couldn't store the token in the {}: {e}",
            kubyl_kube::auth::store::store_name()
        )
    })
}

/// Opens a transport to `install`'s server per the `argocd.api_transport` setting: the service
/// proxy, else (or only) a temporary forward. Checked with the unauthenticated version call.
async fn open_transport(
    cluster: &ClusterId,
    install: &Install,
    cx: &mut AsyncApp,
) -> Result<(ArgoApi, Option<ForwardId>), String> {
    let (client, setting) = cx.update(|cx| {
        (
            ConnectionManager::try_global(cx).and_then(|m| m.read(cx).client(cluster)),
            settings::get(cx).api_transport,
        )
    });
    let client = client.ok_or("the cluster isn't connected")?;
    let server = install
        .server
        .clone()
        .ok_or_else(|| format!("No argocd-server Service found in {}", install.namespace))?;
    let https = !install.insecure && server.https_port.is_some();
    let port = if https {
        server.https_port
    } else {
        server.http_port.or(server.https_port)
    }
    .ok_or("the argocd-server Service has no HTTP(S) port")?;
    let mut proxy_error = None;
    if setting != ApiTransport::Forward {
        let transport = Transport::proxy(
            client.clone(),
            &install.namespace,
            &server.name,
            port,
            https,
            &install.root_path,
        );
        let api = ArgoApi::new(transport, None);
        let probe = run(cx, {
            let api = api.clone();
            async move { api.version().await }
        })
        .await;
        match probe {
            Ok(_) => return Ok((api, None)),
            Err(err) if setting == ApiTransport::Proxy => return Err(err.to_string()),
            Err(err) => {
                tracing::info!("Argo CD through the service proxy: {err}; trying a port-forward");
                proxy_error = Some(err.to_string());
            }
        }
    }
    // A temporary loopback forward, shown in Active Sessions.
    let spec = forward_spec(cluster, install, &server.name, port, https);
    let id = cx.update(|cx| PortForwardManager::start(client, spec, cx));
    let start = Instant::now();
    let local_port = loop {
        let info = cx.update(|cx| PortForwardManager::global(cx).read(cx).info(id));
        match info {
            Some(info) if info.state == ForwardState::Listening && info.local_port != 0 => {
                break info.local_port;
            }
            Some(ForwardInfo {
                state: ForwardState::Failed(err) | ForwardState::Reconnecting(err),
                ..
            }) if start.elapsed() > Duration::from_secs(5) => {
                cx.update(|cx| stop_forward(Some(id), cx));
                return Err(match proxy_error {
                    Some(proxy) => format!("{proxy}; port-forward: {err}"),
                    None => err,
                });
            }
            None => return Err("the port-forward stopped".into()),
            _ => {}
        }
        if start.elapsed() > FORWARD_TIMEOUT {
            cx.update(|cx| stop_forward(Some(id), cx));
            return Err("the port-forward to argocd-server didn't start".into());
        }
        cx.background_executor()
            .timer(Duration::from_millis(100))
            .await;
    };
    let transport =
        Transport::forward(local_port, https, &install.root_path).map_err(|e| e.to_string())?;
    let api = ArgoApi::new(transport, None);
    let probe = run(cx, {
        let api = api.clone();
        async move { api.version().await }
    })
    .await;
    if let Err(err) = probe {
        cx.update(|cx| stop_forward(Some(id), cx));
        return Err(err.to_string());
    }
    Ok((api, Some(id)))
}

fn forward_spec(
    cluster: &ClusterId,
    install: &Install,
    service: &str,
    port: u16,
    https: bool,
) -> ForwardSpec {
    let stop_cluster = cluster.clone();
    ForwardSpec {
        cluster: cluster.clone(),
        namespace: install.namespace.clone(),
        kind: ForwardKind::Service {
            service: service.to_string(),
        },
        port: RemotePort::Service(Some(port)),
        // Loopback only.
        bind_address: "127.0.0.1".into(),
        local_port: 0,
        target: ResourceRef::object(
            cluster.clone(),
            Gvr::new("", "v1", "services"),
            Some(install.namespace.clone()),
            service.to_string(),
        ),
        remote_port: Some(port),
        http: true,
        https,
        open_browser: false,
        ephemeral: Some(Ephemeral {
            title: format!("Argo CD API · svc/{service}:{port}"),
            buttons: Vec::new(),
            on_stop: Arc::new(move |cx| {
                if let Some(argo) = ArgoCd::try_global(cx) {
                    let cluster = stop_cluster.clone();
                    argo.update(cx, |this, cx| this.forward_stopped(&cluster, cx));
                }
            }),
        }),
    }
}

/// Controller namespaces the loaded Applications report (detection hints).
fn controller_namespaces(cluster: &ClusterId, cx: &App) -> Vec<String> {
    // Only namespaces Applications report; detection tries the usual ones itself when it
    // can't list argocd-cm.
    let mut out: Vec<String> = Vec::new();
    for store in ResourceStores::all(cx) {
        let store = store.read(cx);
        let key = store.key();
        if &key.cluster != cluster || key.gvr.group != GROUP || key.gvr.resource != "applications" {
            continue;
        }
        for object in store.objects().values() {
            if let Some(ns) = object
                .pointer("/status/controllerNamespace")
                .and_then(|v| v.as_str())
                && !out.iter().any(|o| o == ns)
            {
                out.push(ns.to_string());
            }
        }
    }
    out
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
