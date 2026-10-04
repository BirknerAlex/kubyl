//! [`ArgoCore`]: Argo CD state per cluster, on any [`Host`]. It knows which Argo CD CRDs a
//! cluster serves, where Argo CD is installed ([`crate::detect`]) and the API-mode session.
//!
//! API mode only ever signs in to an install the user confirmed in the sign-in dialog
//! (namespace, Service and its UID, remembered per context in state.json). A stored token is
//! sent only to that Service; a re-created Service needs a new confirmation. The user's
//! Kubernetes credentials never reach Argo CD: through the service proxy they authenticate to the
//! API server (which strips them), through a forward they aren't sent at all.
//!
//! SSO sessions ([`crate::sso`]) keep their refresh token in the keychain next to the session
//! token and renew themselves when the token expires or the server rejects it. A sign-in with a
//! username and password keeps those there too, and signs in again with them when the session
//! expires; once Argo CD rejects them they're deleted and the dialog asks again.
//!
//! What the service reads from the app comes in as snapshots: [`Link`] (the connection of a
//! cluster), [`ArgoSettings`] and [`ArgoState`]. What it changes in the app goes out as
//! [`ArgoEffect`]s. Forwards to `argocd-server` come from a [`Reach`].

use std::collections::HashMap;
use std::convert::Infallible;
use std::future::Future;
use std::sync::Arc;
use std::time::{Duration, Instant};

use futures::channel::{mpsc, oneshot};
use kubyl_base::host::{Flow, Host, HostExt as _, Pace, Service, TaskHandle};
use kubyl_base::{ArgoCdCaps, ClusterId, Notice};
use kubyl_kube_core::kubeconfig::ContextInfo;
use kubyl_portforward_core::reach::{Reach, ReachPort, ReachRequest, Reached};
use kubyl_resources_core::source::StoreSource;
use secrecy::{ExposeSecret as _, SecretString};
use serde_json::{Value, json};

use crate::api::{ApiError, ArgoApi, Transport, UserInfo};
use crate::detect::{self, Install};
use crate::model::GROUP;
use crate::settings::{self, ApiTransport, ArgoSettings, ArgoState, ContextKey, TrustedInstall};
use crate::sso::{self, SsoConfig, SsoTokens};

/// How long a temporary forward may take to listen.
pub const FORWARD_TIMEOUT: Duration = Duration::from_secs(20);
/// How long a forward may keep reconnecting (its target doesn't resolve) before API mode gives
/// up on it.
pub const FORWARD_RECONNECT_GRACE: Duration = Duration::from_secs(5);
/// While Argo CD is being installed (CRDs first, workloads later), detection finds nothing or
/// no version. It runs again: 3 s doubling, 5 times.
pub const DETECT_RETRY: Duration = Duration::from_secs(3);
pub const DETECT_RETRIES: u32 = 5;
/// An SSO or password session renews itself at most this often after a rejected token.
const RENEW_EVERY: Duration = Duration::from_secs(60);

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
    Sso(Option<mpsc::UnboundedSender<String>>),
}

/// What the service asks the app to do besides what [`Host`] offers.
#[derive(Debug)]
pub enum ArgoEffect {
    /// Installs or versions changed: the explorer's Argo CD group shows them.
    TreeGroupsChanged,
    /// The confirmed installs changed: store them in the app's state.
    SetState(ArgoState),
    /// Open the sign-in URL of an SSO sign-in in the browser.
    OpenUrl(String),
    /// Detection retries after a delay: call [`ArgoCore::detect_again`] with the controller
    /// namespaces the loaded Applications report (see [`controller_namespaces`]), or none.
    DetectAgain(ClusterId),
}

/// What the service knows of a cluster's connection. The app pushes it when the connection
/// changes, and before it calls a method that needs it.
#[derive(Clone, Default)]
pub struct Link {
    /// The client of a connected cluster.
    pub client: Option<kube::Client>,
    /// The kubeconfig entry, which keys the confirmed install and its token.
    pub context: Option<ContextInfo>,
}

/// Pieces of an API-mode session that work in flight reports back. Short-lived messages in a
/// channel: the size difference between variants doesn't matter.
#[allow(clippy::large_enum_variant)]
enum Step {
    /// The transport to the server works.
    Api(ArgoApi),
    /// A forward carries the transport. The answer says whether the session took it.
    Forward(Reached, oneshot::Sender<bool>),
    /// Whether a rejected token can renew itself.
    Renewable(bool),
    /// Asks whether the session still exists.
    Alive(oneshot::Sender<bool>),
    /// An SSO sign-in wants the browser opened at this URL.
    OpenUrl(String),
    /// A sign-in checked out and stored its tokens.
    SignedIn {
        api: ArgoApi,
        renewable: bool,
        install: Install,
        username: String,
        version: String,
        via: &'static str,
    },
    /// A reconnect ended.
    Connected(Result<ApiState, String>),
    /// A sign-in ended.
    SignedInResult(Result<UserInfo, String>),
}

/// Sends steps from work in flight.
#[derive(Clone)]
struct Steps(mpsc::UnboundedSender<Step>);

impl Steps {
    fn send(&self, step: Step) {
        self.0.unbounded_send(step).ok();
    }

    async fn ask(&self, make: impl FnOnce(oneshot::Sender<bool>) -> Step) -> bool {
        let (reply, answer) = oneshot::channel();
        self.send(make(reply));
        answer.await.unwrap_or(false)
    }
}

/// Work in flight and the handler of its steps. Dropping it cancels both.
struct Job {
    _work: TaskHandle,
    _steps: TaskHandle,
}

/// A forward that carries a session's transport. Dropping it stops the forward.
struct Forward {
    id: u64,
    _reached: Reached,
    watch: TaskHandle,
}

impl Forward {
    /// Drops the forward from inside its own watch's callback.
    fn release_from_watch(self) {
        self.watch.detach();
    }
}

struct ApiSession {
    state: ApiState,
    install: Install,
    /// The transport (with the token once connected).
    api: Option<ArgoApi>,
    forward: Option<Forward>,
    /// An SSO session with a refresh token: a rejected token renews it.
    renewable: bool,
    /// When the session last reconnected after a rejected token.
    renewed_at: Option<Instant>,
    _job: Option<Job>,
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
            _job: None,
        }
    }
}

#[derive(Default)]
struct ClusterArgo {
    caps: ArgoCdCaps,
    link: Link,
    detection: Detection,
    _detect_task: Option<TaskHandle>,
    detect_retries: u32,
    _retry_task: Option<TaskHandle>,
    api: Option<ApiSession>,
    /// A sign-in in progress. Owned here, not by the dialog: an SSO sign-in waits for the
    /// browser and must survive the dialog closing.
    _sign_in: Option<Job>,
}

/// Argo CD state of every cluster.
pub struct ArgoCore {
    clusters: HashMap<ClusterId, ClusterArgo>,
    reach: Arc<dyn Reach>,
    settings: ArgoSettings,
    state: ArgoState,
    /// Numbers the forwards, so a stopped one is told apart from its successor.
    forwards: u64,
}

impl Service for ArgoCore {
    type Event = Infallible;
    type Effect = ArgoEffect;
}

impl ArgoCore {
    pub fn new(reach: Arc<dyn Reach>) -> Self {
        Self {
            clusters: HashMap::new(),
            reach,
            settings: ArgoSettings::default(),
            state: ArgoState::default(),
            forwards: 0,
        }
    }

    // ----- Inputs -----

    pub fn set_settings(&mut self, settings: ArgoSettings) {
        self.settings = settings;
    }

    /// The installs the user confirmed (state.json).
    pub fn set_state(&mut self, state: ArgoState) {
        self.state = state;
    }

    pub fn state(&self) -> &ArgoState {
        &self.state
    }

    /// The connection of `cluster`.
    pub fn set_link(&mut self, cluster: &ClusterId, link: Link) {
        self.clusters.entry(cluster.clone()).or_default().link = link;
    }

    /// The clusters the service has an entry for.
    pub fn clusters(&self) -> Vec<ClusterId> {
        self.clusters.keys().cloned().collect()
    }

    /// The key of a cluster entry across restarts.
    pub fn context_key(&self, cluster: &ClusterId) -> Option<ContextKey> {
        let info = self.clusters.get(cluster)?.link.context.as_ref()?;
        Some(ContextKey::for_entry(info, &self.state))
    }

    /// The confirmed install of a cluster.
    pub fn trusted(&self, cluster: &ClusterId) -> Option<TrustedInstall> {
        let key = self.context_key(cluster)?;
        self.state
            .trusted
            .iter()
            .find(|t| t.context == key)
            .cloned()
    }

    // ----- Following the cluster -----

    /// Follows a cluster's caps and connection: starts detection when CRDs appear, drops
    /// everything when they go or the cluster disconnects. `hints` are the controller
    /// namespaces the loaded Applications report ([`controller_namespaces`]).
    pub fn sync_cluster(
        &mut self,
        cluster: &ClusterId,
        caps: ArgoCdCaps,
        connected: bool,
        link: Link,
        hints: Vec<String>,
        host: &mut dyn Host<Self>,
    ) {
        let entry = self.clusters.entry(cluster.clone()).or_default();
        entry.link = link;
        let changed = entry.caps != caps;
        entry.caps = caps;
        if !connected || !caps.any() {
            let had = entry.api.is_some() || entry.detection != Detection::Idle;
            entry.api = None;
            entry.detection = Detection::Idle;
            entry._detect_task = None;
            entry._retry_task = None;
            entry._sign_in = None;
            if had || changed {
                host.effect(ArgoEffect::TreeGroupsChanged);
                host.notify();
            }
            return;
        }
        if changed {
            // CRDs came or went: look again.
            entry.detection = Detection::Idle;
            entry.detect_retries = 0;
            host.effect(ArgoEffect::TreeGroupsChanged);
            host.notify();
        }
        if entry.detection == Detection::Idle {
            self.detect(cluster, hints, host);
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
    pub fn redetect(&mut self, cluster: &ClusterId, hints: Vec<String>, host: &mut dyn Host<Self>) {
        if let Some(entry) = self.clusters.get_mut(cluster) {
            entry.detection = Detection::Idle;
            entry.detect_retries = 0;
        }
        self.detect(cluster, hints, host);
    }

    /// Runs a retry the service asked for with [`ArgoEffect::DetectAgain`].
    pub fn detect_again(
        &mut self,
        cluster: &ClusterId,
        hints: Vec<String>,
        host: &mut dyn Host<Self>,
    ) {
        // The timer that asked is finishing: let it, instead of cancelling itself.
        if let Some(retry) = self
            .clusters
            .get_mut(cluster)
            .and_then(|c| c._retry_task.take())
        {
            retry.detach();
        }
        self.detect(cluster, hints, host);
    }

    fn detect(&mut self, cluster: &ClusterId, hints: Vec<String>, host: &mut dyn Host<Self>) {
        let Some(entry) = self.clusters.get_mut(cluster) else {
            return;
        };
        let Some(client) = entry.link.client.clone() else {
            return;
        };
        // A retry keeps showing what the last run found.
        if !matches!(entry.detection, Detection::Done(_)) {
            entry.detection = Detection::Running;
        }
        entry._retry_task = None;
        let id = cluster.clone();
        entry._detect_task = Some(host.spawn(
            detect::detect(client, hints),
            move |this, result, host| {
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
                    entry._retry_task = Some(host.after(delay, move |_, host| {
                        host.effect(ArgoEffect::DetectAgain(retry));
                    }));
                }
                host.effect(ArgoEffect::TreeGroupsChanged);
                host.notify();
                this.auto_connect(&id, host);
            },
        ));
        host.notify();
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
    pub fn trusted_install(&self, cluster: &ClusterId) -> Option<Install> {
        let trusted = self.trusted(cluster)?;
        self.installs(cluster)
            .iter()
            .find(|i| trusted.matches(i))
            .cloned()
    }

    /// Why the confirmed install can't be used any more (a re-created Service).
    pub fn trust_problem(&self, cluster: &ClusterId) -> Option<String> {
        let trusted = self.trusted(cluster)?;
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

    fn auto_connect(&mut self, cluster: &ClusterId, host: &mut dyn Host<Self>) {
        let already = self.clusters.get(cluster).is_some_and(|c| c.api.is_some());
        if already || !self.settings.auto_connect {
            return;
        }
        if self.trusted_install(cluster).is_some() {
            self.connect(cluster, host);
        }
    }

    fn env(&self, cluster: &ClusterId) -> Env {
        Env {
            reach: self.reach.clone(),
            client: self
                .clusters
                .get(cluster)
                .and_then(|c| c.link.client.clone()),
            setting: self.settings.api_transport,
        }
    }

    /// Connects API mode to the confirmed install with the stored token (no-op without one).
    pub fn connect(&mut self, cluster: &ClusterId, host: &mut dyn Host<Self>) {
        let Some(install) = self.trusted_install(cluster) else {
            return;
        };
        let Some(key) = self.context_key(cluster) else {
            return;
        };
        let env = self.env(cluster);
        let Some(entry) = self.clusters.get_mut(cluster) else {
            return;
        };
        // The old session's forward stops with it.
        drop(entry.api.take());
        let mut session = ApiSession::new(install.clone());
        let id = cluster.clone();
        session._job = Some(start_job(
            host,
            {
                let id = id.clone();
                move |steps| connect_work(env, id, install, key, steps)
            },
            move |this, step, host| {
                if let Step::Connected(result) = step {
                    this.connect_done(&id, result, host);
                    return Flow::Stop;
                }
                this.apply_step(&id, step, host);
                Flow::Continue
            },
        ));
        entry.api = Some(session);
        host.notify();
    }

    fn connect_done(
        &mut self,
        cluster: &ClusterId,
        result: Result<ApiState, String>,
        host: &mut dyn Host<Self>,
    ) {
        if let Some(session) = self.session_mut(cluster) {
            session.state = match result {
                Ok(state) => state,
                Err(err) => ApiState::Failed(err),
            };
        }
        host.notify();
    }

    /// Applies what work in flight reports.
    fn apply_step(&mut self, cluster: &ClusterId, step: Step, host: &mut dyn Host<Self>) {
        match step {
            Step::Api(api) => {
                if let Some(session) = self.session_mut(cluster) {
                    session.api = Some(api);
                }
            }
            Step::Forward(mut reached, reply) => {
                // Recorded at once: a sign-out, reconnect or cancelled job then stops it with
                // its session instead of leaking it. Without room for it, it stops right here.
                let free = self
                    .session_mut(cluster)
                    .is_some_and(|session| session.forward.is_none());
                if free {
                    self.forwards += 1;
                    let id = self.forwards;
                    let stopped = reached.take_stopped();
                    let stopped_cluster = cluster.clone();
                    let watch = host.spawn(stopped, move |this, (), host| {
                        this.forward_stopped(&stopped_cluster, id, host)
                    });
                    if let Some(session) = self.session_mut(cluster) {
                        session.forward = Some(Forward {
                            id,
                            _reached: reached,
                            watch,
                        });
                    }
                }
                reply.send(free).ok();
            }
            Step::Renewable(renewable) => {
                if let Some(session) = self.session_mut(cluster) {
                    session.renewable = renewable;
                }
            }
            Step::Alive(reply) => {
                reply.send(self.session_mut(cluster).is_some()).ok();
            }
            Step::OpenUrl(url) => host.effect(ArgoEffect::OpenUrl(url)),
            Step::SignedIn {
                api,
                renewable,
                install,
                username,
                version,
                via,
            } => {
                self.trust(cluster, &install, Some(username.clone()), host);
                if let Some(session) = self.session_mut(cluster) {
                    session.renewable = renewable;
                    session.api = Some(api);
                    session.state = ApiState::Connected {
                        user: username,
                        version,
                        via,
                    };
                }
                host.notify();
            }
            Step::Connected(_) | Step::SignedInResult(_) => {}
        }
    }

    // ----- Sessions -----

    fn session_mut(&mut self, cluster: &ClusterId) -> Option<&mut ApiSession> {
        self.clusters.get_mut(cluster)?.api.as_mut()
    }

    /// The session token for a web view of `namespace/service`: only for the argocd-server
    /// Service of the confirmed install, and only while API mode is signed in there.
    pub fn web_session_token(
        &self,
        cluster: &ClusterId,
        namespace: &str,
        service: &str,
    ) -> Option<SecretString> {
        let session = self.clusters.get(cluster)?.api.as_ref()?;
        if !session.state.is_connected() {
            return None;
        }
        let trusted = self.trusted_install(cluster)?;
        let server = trusted.server.as_ref()?;
        let ours = trusted.namespace == namespace
            && server.name == service
            && session.install.namespace == namespace;
        if !ours {
            return None;
        }
        session.api.as_ref()?.token().cloned()
    }

    /// Remembers `install` as the confirmed one of its cluster (one per cluster).
    fn trust(
        &mut self,
        cluster: &ClusterId,
        install: &Install,
        username: Option<String>,
        host: &mut dyn Host<Self>,
    ) {
        let (Some(key), Some(server)) = (self.context_key(cluster), install.server.clone()) else {
            return;
        };
        self.state.trusted.retain(|t| t.context != key);
        self.state.trusted.push(TrustedInstall {
            context: key,
            namespace: install.namespace.clone(),
            service: server.name,
            uid: server.uid,
            username,
        });
        host.effect(ArgoEffect::SetState(self.state.clone()));
    }

    /// Forgets the confirmed install of a cluster.
    fn forget(&mut self, cluster: &ClusterId, host: &mut dyn Host<Self>) {
        let Some(key) = self.context_key(cluster) else {
            return;
        };
        self.state.trusted.retain(|t| t.context != key);
        host.effect(ArgoEffect::SetState(self.state.clone()));
    }

    /// Signs in to `install` (the user confirmed it in the dialog) and remembers it. The answer
    /// arrives on the returned channel; it closes without one when the sign-in is cancelled.
    pub fn sign_in(
        &mut self,
        cluster: &ClusterId,
        install: Install,
        credentials: Credentials,
        host: &mut dyn Host<Self>,
    ) -> oneshot::Receiver<Result<UserInfo, String>> {
        let (done, answer) = oneshot::channel();
        let Some(key) = self.context_key(cluster) else {
            done.send(Err("unknown context".into())).ok();
            return answer;
        };
        let Some(service) = install.server.as_ref().map(|s| s.name.clone()) else {
            done.send(Err(format!(
                "No argocd-server Service found in {}",
                install.namespace
            )))
            .ok();
            return answer;
        };
        let env = self.env(cluster);
        let entry = self.clusters.entry(cluster.clone()).or_default();
        // Reuse a transport to the same install; otherwise start over (the old session's
        // forward stops with it).
        let reuse = entry
            .api
            .as_ref()
            .filter(|s| s.install == install)
            .and_then(|s| s.api.clone());
        if reuse.is_none() {
            drop(entry.api.take());
        }
        match entry.api.as_mut() {
            Some(session) => session.state = ApiState::Connecting,
            None => entry.api = Some(ApiSession::new(install.clone())),
        }
        host.notify();
        let id = cluster.clone();
        let mut done = Some(done);
        // Replaces (cancels) a sign-in still waiting for the browser.
        entry._sign_in = Some(start_job(
            host,
            {
                let (id, install) = (id.clone(), install.clone());
                move |steps| sign_in_work(env, id, install, key, service, credentials, reuse, steps)
            },
            move |this, step, host| {
                if let Step::SignedInResult(result) = step {
                    match &result {
                        Ok(info) => host.toast(Notice::info(format!(
                            "Signed in to Argo CD as {}",
                            info.username
                        ))),
                        Err(err) => {
                            if let Some(session) = this.session_mut(&id) {
                                session.state = ApiState::SignInRequired(Some(err.clone()));
                            }
                        }
                    }
                    host.notify();
                    if let Some(done) = done.take() {
                        done.send(result).ok();
                    }
                    return Flow::Stop;
                }
                this.apply_step(&id, step, host);
                Flow::Continue
            },
        ));
        answer
    }

    /// Signs out: ends the session on the server (the token is revoked, so a web view's copy
    /// of it stops working too), forgets the token, stops the forward; Kubernetes mode stays.
    pub fn sign_out(
        &mut self,
        cluster: &ClusterId,
        forget_install: bool,
        host: &mut dyn Host<Self>,
    ) {
        let key = self.context_key(cluster);
        let session = self.clusters.get_mut(cluster).and_then(|c| {
            // A sign-in still under way is cancelled, or it would store its token afterwards.
            c._sign_in = None;
            c.api.take()
        });
        if let Some(session) = &session
            && let (Some(key), Some(server)) = (key, &session.install.server)
        {
            let token_key = settings::token_key(&key, &session.install.namespace, &server.name);
            host.spawn(
                async move {
                    tokio::task::spawn_blocking(move || {
                        kubyl_kube_core::auth::store::delete(&refresh_key(&token_key)).ok();
                        kubyl_kube_core::auth::store::delete(&password_key(&token_key)).ok();
                        kubyl_kube_core::auth::store::delete(&token_key)
                    })
                    .await
                    .ok();
                },
                |_, (), _| {},
            )
            .detach();
        }
        if let Some(session) = session {
            let ApiSession {
                api,
                state,
                forward,
                ..
            } = session;
            match api.filter(|_| state.is_connected()) {
                Some(api) => {
                    // The forward carries the logout, then stops.
                    host.spawn(async move { api.logout().await }, move |_, result, _| {
                        if let Err(err) = result {
                            tracing::info!("Argo CD logout failed: {err}");
                        }
                        drop(forward);
                    })
                    .detach();
                }
                None => drop(forward),
            }
        }
        if forget_install {
            self.forget(cluster, host);
        }
        host.notify();
    }

    /// An API call said the token is no longer valid. An SSO session renews itself, a password
    /// session signs in again (once a minute at most); otherwise the user signs in again.
    pub fn unauthorized(&mut self, cluster: &ClusterId, host: &mut dyn Host<Self>) {
        let renew = self
            .session_mut(cluster)
            .is_some_and(|s| s.renewable && s.renewed_at.is_none_or(|t| t.elapsed() > RENEW_EVERY));
        if renew {
            self.connect(cluster, host);
            if let Some(session) = self.session_mut(cluster) {
                session.renewed_at = Some(Instant::now());
            }
            return;
        }
        if let Some(session) = self.session_mut(cluster) {
            session.state = ApiState::SignInRequired(Some(
                "The Argo CD session expired. Sign in again.".into(),
            ));
            host.notify();
        }
    }

    /// The user stopped API mode's forward in Active Sessions.
    fn forward_stopped(&mut self, cluster: &ClusterId, id: u64, host: &mut dyn Host<Self>) {
        // A leftover forward of an earlier session says nothing about the current one.
        let Some(session) = self.session_mut(cluster) else {
            return;
        };
        if session.forward.as_ref().is_none_or(|f| f.id != id) {
            return;
        }
        if let Some(forward) = session.forward.take() {
            forward.release_from_watch();
        }
        session.api = None;
        session.state = ApiState::Failed("The port-forward to argocd-server was stopped.".into());
        host.notify();
    }
}

/// What work in flight needs from the app, copied out when it starts.
struct Env {
    reach: Arc<dyn Reach>,
    client: Option<kube::Client>,
    setting: ApiTransport,
}

/// Runs `work` on Tokio with a channel for its steps, and `handler` on the host's thread for
/// each step until it says stop.
fn start_job<F>(
    host: &mut dyn Host<ArgoCore>,
    work: impl FnOnce(Steps) -> F,
    mut handler: impl FnMut(&mut ArgoCore, Step, &mut dyn Host<ArgoCore>) -> Flow + 'static,
) -> Job
where
    F: Future<Output = ()> + Send + 'static,
{
    let (tx, rx) = mpsc::unbounded();
    let steps = host.batches(rx, Pace::IMMEDIATE, move |this, batch: Vec<Step>, host| {
        for step in batch {
            if handler(this, step, host) == Flow::Stop {
                return Flow::Stop;
            }
        }
        Flow::Continue
    });
    let work = host.spawn(work(Steps(tx)), |_, (), _| {});
    Job {
        _work: work,
        _steps: steps,
    }
}

/// Where an SSO session's refresh token is kept, next to its session token.
pub fn refresh_key(token_key: &str) -> String {
    format!("{token_key}/refresh")
}

/// Where a password session's username and password are kept, next to its session token.
pub fn password_key(token_key: &str) -> String {
    format!("{token_key}/password")
}

/// A local account's username and password, kept to sign in again when the session expires.
struct SavedPassword {
    username: String,
    password: SecretString,
}

impl SavedPassword {
    fn to_secret(&self) -> SecretString {
        SecretString::from(
            json!({"username": self.username, "password": self.password.expose_secret()})
                .to_string(),
        )
    }

    fn parse(secret: &SecretString) -> Option<Self> {
        let value: Value = serde_json::from_str(secret.expose_secret()).ok()?;
        Some(Self {
            username: value["username"].as_str()?.to_string(),
            password: SecretString::from(value["password"].as_str()?.to_string()),
        })
    }
}

async fn read_token(key: String) -> Option<SecretString> {
    tokio::task::spawn_blocking(move || kubyl_kube_core::auth::store::get(&key))
        .await
        .ok()
        .and_then(|r| r.inspect_err(|e| tracing::warn!("keychain: {e}")).ok())
        .flatten()
}

async fn delete_token(key: String) {
    tokio::task::spawn_blocking(move || kubyl_kube_core::auth::store::delete(&key))
        .await
        .ok();
}

async fn write_token(key: String, token: SecretString) -> Result<(), String> {
    tokio::task::spawn_blocking(move || kubyl_kube_core::auth::store::set(&key, &token))
        .await
        .map_err(|e| e.to_string())?
        .map_err(|e| {
            format!(
                "couldn't store the token in the {}: {e}",
                kubyl_kube_core::auth::store::store_name()
            )
        })
}

/// Signs in again with the saved username and password and stores the new token. Rejected
/// credentials are deleted, so the dialog asks again.
async fn relogin(
    api: &ArgoApi,
    token_key: &str,
    saved: &SavedPassword,
) -> Result<SecretString, ApiState> {
    match api.login(&saved.username, &saved.password).await {
        Ok(token) => {
            if let Err(err) = write_token(token_key.to_string(), token.clone()).await {
                tracing::info!("Argo CD session token not stored: {err}");
            }
            Ok(token)
        }
        Err(ApiError::InvalidCredentials) => {
            delete_token(password_key(token_key)).await;
            delete_token(token_key.to_string()).await;
            Err(ApiState::SignInRequired(Some(format!(
                "Argo CD no longer takes the saved password for {}. Sign in again.",
                saved.username
            ))))
        }
        Err(err) => Err(ApiState::Failed(err.to_string())),
    }
}

/// Stores an SSO session: the refresh token, and the session token when the keychain takes it
/// (Windows Credential Manager holds 2.5 KB; without it the next start renews the session).
async fn store_session(token_key: &str, tokens: &SsoTokens) -> Result<(), String> {
    let Some(refresh) = &tokens.refresh_token else {
        delete_token(refresh_key(token_key)).await;
        return write_token(token_key.to_string(), tokens.id_token.clone()).await;
    };
    write_token(refresh_key(token_key), refresh.clone()).await?;
    if let Err(err) = write_token(token_key.to_string(), tokens.id_token.clone()).await {
        tracing::info!("Argo CD session token not stored: {err}");
    }
    Ok(())
}

/// Renews an SSO session with its refresh token (settings from the confirmed server say where)
/// and stores the new tokens.
async fn renew(
    api: &ArgoApi,
    token_key: &str,
    refresh: &SecretString,
) -> Result<SecretString, String> {
    let settings = api.settings().await.map_err(|e| e.to_string())?;
    let config = SsoConfig::from_settings(&settings)?;
    let tokens = sso::refresh(&config, refresh).await?;
    store_session(token_key, &tokens).await?;
    Ok(tokens.id_token)
}

/// Opens a transport to `install`'s server per the `argocd.api_transport` setting: the service
/// proxy, else (or only) a temporary forward. Checked with the unauthenticated version call.
/// The forward, when there is one, goes to the session.
async fn open_transport(
    env: &Env,
    cluster: &ClusterId,
    install: &Install,
    steps: &Steps,
) -> Result<ArgoApi, String> {
    let client = env.client.clone().ok_or("the cluster isn't connected")?;
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
    if env.setting != ApiTransport::Forward {
        let transport = Transport::proxy(
            client.clone(),
            &install.namespace,
            &server.name,
            port,
            https,
            &install.root_path,
        );
        let api = ArgoApi::new(transport, None);
        match api.version().await {
            Ok(_) => return Ok(api),
            Err(err) if env.setting == ApiTransport::Proxy => return Err(err.to_string()),
            Err(err) => {
                tracing::info!("Argo CD through the service proxy: {err}; trying a port-forward");
                proxy_error = Some(err.to_string());
            }
        }
    }
    // A temporary loopback forward, shown in Active Sessions.
    let reached = env
        .reach
        .reach(ReachRequest {
            client,
            cluster: cluster.clone(),
            namespace: install.namespace.clone(),
            service: server.name.clone(),
            port: ReachPort::Number(port),
            title: format!("Argo CD API · svc/{}:{port}", server.name),
            https,
            timeout: FORWARD_TIMEOUT,
            reconnect_grace: Some(FORWARD_RECONNECT_GRACE),
        })
        .await
        .map_err(|err| match &proxy_error {
            Some(proxy) => format!("{proxy}; {err}"),
            None => err,
        })?;
    let transport = Transport::forward(reached.local_port, https, &install.root_path)
        .map_err(|e| e.to_string())?;
    let api = ArgoApi::new(transport, None);
    // A forward that doesn't answer stops with `reached`.
    api.version().await.map_err(|e| e.to_string())?;
    // The session keeps it, so a sign-out, reconnect or cancelled job stops it.
    if !steps.ask(|reply| Step::Forward(reached, reply)).await {
        return Err("The session ended while connecting.".into());
    }
    Ok(api)
}

/// Reconnects API mode with the stored token: renews an expiring SSO session, signs in again
/// with a saved password, and checks the token.
async fn connect_work(
    env: Env,
    cluster: ClusterId,
    install: Install,
    key: ContextKey,
    steps: Steps,
) {
    let result = async {
        let api = open_transport(&env, &cluster, &install, &steps).await?;
        steps.send(Step::Api(api.clone()));
        let version = api.version().await.map_err(|e| e.to_string())?;
        let service = install
            .server
            .as_ref()
            .map(|s| s.name.clone())
            .unwrap_or_default();
        let token_key = settings::token_key(&key, &install.namespace, &service);
        let mut token = read_token(token_key.clone()).await;
        let refresh = read_token(refresh_key(&token_key)).await;
        let saved = read_token(password_key(&token_key))
            .await
            .and_then(|s| SavedPassword::parse(&s));
        steps.send(Step::Renewable(refresh.is_some() || saved.is_some()));
        // An SSO session renews itself before it expires, and once if it's rejected.
        let mut renewed = false;
        if let Some(refresh) = &refresh
            && token.as_ref().is_none_or(sso::expiring)
        {
            renewed = true;
            match renew(&api, &token_key, refresh).await {
                Ok(fresh) => token = Some(fresh),
                Err(err) => tracing::info!("Argo CD session renewal failed: {err}"),
            }
        }
        // A password session signs in again once its token is gone or rejected.
        let mut relogged = false;
        if token.is_none()
            && let Some(saved) = &saved
        {
            relogged = true;
            match relogin(&api, &token_key, saved).await {
                Ok(fresh) => token = Some(fresh),
                Err(state) => return Ok(state),
            }
        }
        let Some(mut token) = token else {
            return Ok::<_, String>(ApiState::SignInRequired(None));
        };
        loop {
            let signed = api.with_token(token.clone());
            match signed.user_info().await {
                Ok(info) => {
                    let via = api.transport().describe();
                    steps.send(Step::Api(signed));
                    return Ok(ApiState::Connected {
                        user: info.username,
                        version,
                        via,
                    });
                }
                Err(ApiError::Unauthorized) if !renewed && refresh.is_some() => {
                    renewed = true;
                    let refresh = refresh.as_ref().expect("checked");
                    match renew(&api, &token_key, refresh).await {
                        Ok(fresh) => token = fresh,
                        Err(err) => {
                            return Ok(ApiState::SignInRequired(Some(format!(
                                "The SSO session couldn't be renewed ({err}). Sign in again."
                            ))));
                        }
                    }
                }
                Err(ApiError::Unauthorized) if !relogged && saved.is_some() => {
                    relogged = true;
                    let saved = saved.as_ref().expect("checked");
                    match relogin(&api, &token_key, saved).await {
                        Ok(fresh) => token = fresh,
                        Err(state) => return Ok(state),
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
    steps.send(Step::Connected(result));
}

/// Signs in with `credentials`, checks the token and stores it.
#[allow(clippy::too_many_arguments)]
async fn sign_in_work(
    env: Env,
    cluster: ClusterId,
    install: Install,
    key: ContextKey,
    service: String,
    credentials: Credentials,
    reuse: Option<ArgoApi>,
    steps: Steps,
) {
    let result = async {
        let api = match reuse {
            Some(api) => api,
            None => open_transport(&env, &cluster, &install, &steps).await?,
        };
        steps.send(Step::Api(api.clone()));
        let mut saved = None;
        let (token, session) = match credentials {
            Credentials::Token(token) => (token, None),
            Credentials::Password { username, password } => {
                let token = api
                    .login(&username, &password)
                    .await
                    .map_err(|e| e.to_string())?;
                saved = Some(SavedPassword { username, password });
                (token, None)
            }
            Credentials::Sso(urls) => {
                let settings = api.settings().await.map_err(|e| e.to_string())?;
                let config = SsoConfig::from_settings(&settings)?;
                let opener = steps.clone();
                let tokens = sso::sign_in(&config, move |url| {
                    opener.send(Step::OpenUrl(url.clone()));
                    if let Some(urls) = urls {
                        urls.unbounded_send(url).ok();
                    }
                })
                .await?;
                (tokens.id_token.clone(), Some(tokens))
            }
        };
        let signed = api.with_token(token.clone());
        let info = signed.user_info().await.map_err(|e| e.to_string())?;
        let version = signed.version().await.unwrap_or_default();
        // Signed out (or reconnected) while the sign-in was under way: store and trust nothing.
        if !steps.ask(Step::Alive).await {
            return Err("The sign-in was cancelled.".to_string());
        }
        let token_key = settings::token_key(&key, &install.namespace, &service);
        let renewable = session.as_ref().is_some_and(|s| s.refresh_token.is_some());
        match &session {
            Some(session) => store_session(&token_key, session).await?,
            None => {
                write_token(token_key.clone(), token).await?;
                // A refresh token of an earlier SSO session would renew the wrong one.
                delete_token(refresh_key(&token_key)).await;
            }
        }
        // The username and password sign in again when the session expires; another way of
        // signing in replaces them.
        match &saved {
            Some(saved) => {
                if let Err(err) = write_token(password_key(&token_key), saved.to_secret()).await {
                    tracing::info!("Argo CD password not stored: {err}");
                }
            }
            None => delete_token(password_key(&token_key)).await,
        }
        let renewable = renewable || saved.is_some();
        let via = api.transport().describe();
        steps.send(Step::SignedIn {
            api: signed,
            renewable,
            install: install.clone(),
            username: info.username.clone(),
            version,
            via,
        });
        Ok::<_, String>(info)
    }
    .await;
    steps.send(Step::SignedInResult(result));
}

/// The namespaces Applications report as their controller's, in the watch caches the app holds
/// (detection hints). Only namespaces Applications report; detection tries the usual ones
/// itself when it can't list argocd-cm.
pub fn controller_namespaces(cluster: &ClusterId, stores: &dyn StoreSource) -> Vec<String> {
    let mut out: Vec<String> = Vec::new();
    for key in stores.keys() {
        if &key.cluster != cluster || key.gvr.group != GROUP || key.gvr.resource != "applications" {
            continue;
        }
        let Some(store) = stores.view(&key) else {
            continue;
        };
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

#[cfg(test)]
mod tests {
    use kubyl_base::host::TestHost;
    use kubyl_kube_core::kubeconfig::{self, fixtures};
    use kubyl_resources_core::source::MemoryStores;
    use kubyl_resources_core::store::StoreKey;

    use super::*;
    use crate::detect::ServerService;

    /// No forwards in these tests.
    struct Refuses;

    impl Reach for Refuses {
        fn reach(
            &self,
            _: ReachRequest,
        ) -> futures::future::BoxFuture<'static, Result<Reached, String>> {
            Box::pin(async { Err("no forwards".to_string()) })
        }
    }

    fn install(namespace: &str, uid: &str) -> Install {
        Install {
            namespace: namespace.into(),
            server: Some(ServerService {
                name: "argocd-server".into(),
                uid: uid.into(),
                https_port: Some(443),
                http_port: Some(80),
            }),
            ..Default::default()
        }
    }

    /// A core that follows the `kind-dev` context of the kubeconfig fixture.
    struct Fixture {
        core: ArgoCore,
        host: TestHost<ArgoCore>,
        cluster: ClusterId,
        context: ContextInfo,
        _dir: tempfile::TempDir,
    }

    fn fixture() -> Fixture {
        let dir = tempfile::tempdir().unwrap();
        let file = dir.path().join("config");
        std::fs::write(&file, fixtures::DEV).unwrap();
        let specs = kubeconfig::source_specs(
            &kubyl_kube_core::settings::KubeSettings {
                load_default_kubeconfig: false,
                kubeconfigs: vec![file.display().to_string()],
                ..Default::default()
            },
            None,
            None,
            &dir.path().join("pasted"),
        );
        let context = kubeconfig::load(&specs)
            .contexts
            .into_iter()
            .find(|c| c.context == "kind-dev")
            .unwrap();
        let cluster = context.id.clone();
        let mut core = ArgoCore::new(Arc::new(Refuses));
        core.set_link(
            &cluster,
            Link {
                client: None,
                context: Some(context.clone()),
            },
        );
        Fixture {
            core,
            host: TestHost::new(),
            cluster,
            context,
            _dir: dir,
        }
    }

    impl Fixture {
        /// The user confirmed `install` for this context.
        fn trust(&mut self, install: &Install) {
            let key = self.core.context_key(&self.cluster).unwrap();
            let server = install.server.clone().unwrap();
            self.core.set_state(ArgoState {
                trusted: vec![TrustedInstall {
                    context: key,
                    namespace: install.namespace.clone(),
                    service: server.name,
                    uid: server.uid,
                    username: Some("admin".into()),
                }],
            });
        }

        fn detected(&mut self, installs: Vec<Install>) {
            let entry = self.core.clusters.get_mut(&self.cluster).unwrap();
            entry.detection = Detection::Done(Arc::new(installs));
        }

        fn session(&mut self, state: ApiState) {
            let mut session = ApiSession::new(install("argocd", "u1"));
            session.state = state;
            self.core.clusters.get_mut(&self.cluster).unwrap().api = Some(session);
        }
    }

    #[test]
    fn only_the_confirmed_service_is_trusted_and_a_recreated_one_is_reported() {
        let mut f = fixture();
        let original = install("argocd", "u1");
        f.trust(&original);

        f.detected(vec![original.clone()]);
        assert_eq!(f.core.trusted_install(&f.cluster), Some(original));
        assert_eq!(f.core.trust_problem(&f.cluster), None);

        f.detected(vec![install("argocd", "u2")]);
        assert_eq!(f.core.trusted_install(&f.cluster), None);
        assert!(
            f.core
                .trust_problem(&f.cluster)
                .unwrap()
                .contains("re-created")
        );

        f.detected(vec![install("elsewhere", "u3")]);
        assert!(
            f.core
                .trust_problem(&f.cluster)
                .unwrap()
                .contains("isn't there any more")
        );
    }

    #[test]
    fn signing_out_ends_the_session_and_can_forget_the_confirmed_install() {
        let mut f = fixture();
        f.trust(&install("argocd", "u1"));
        f.session(ApiState::SignInRequired(None));

        f.core.sign_out(&f.cluster, false, &mut f.host);
        assert_eq!(f.core.api_state(&f.cluster), ApiState::Off);
        assert!(f.core.trusted(&f.cluster).is_some());

        f.core.sign_out(&f.cluster, true, &mut f.host);
        assert!(f.core.trusted(&f.cluster).is_none());
        assert!(
            f.host
                .effects
                .iter()
                .any(|e| matches!(e, ArgoEffect::SetState(state) if state.trusted.is_empty()))
        );
    }

    #[test]
    fn a_rejected_token_asks_for_a_new_sign_in_unless_the_session_renews() {
        let mut f = fixture();
        let connected = ApiState::Connected {
            user: "admin".into(),
            version: "v3".into(),
            via: "test",
        };
        f.session(connected.clone());
        f.core.unauthorized(&f.cluster, &mut f.host);
        assert!(matches!(
            f.core.api_state(&f.cluster),
            ApiState::SignInRequired(Some(_))
        ));

        // A session that can renew tries that first, at most once a minute.
        f.session(connected);
        f.core.session_mut(&f.cluster).unwrap().renewable = true;
        f.core.unauthorized(&f.cluster, &mut f.host);
        assert!(f.core.session_mut(&f.cluster).unwrap().renewed_at.is_some());
    }

    #[test]
    fn a_forward_stopped_from_outside_fails_the_session_unless_it_is_an_old_one() {
        let mut f = fixture();
        f.session(ApiState::Connecting);
        let (reached, _end) = Reached::fake(4000);
        f.core.session_mut(&f.cluster).unwrap().forward = Some(Forward {
            id: 2,
            _reached: reached,
            watch: TaskHandle::none(),
        });

        // A leftover forward of an earlier session says nothing about this one.
        f.core.forward_stopped(&f.cluster, 1, &mut f.host);
        assert_eq!(f.core.api_state(&f.cluster), ApiState::Connecting);

        f.core.forward_stopped(&f.cluster, 2, &mut f.host);
        assert_eq!(
            f.core.api_state(&f.cluster),
            ApiState::Failed("The port-forward to argocd-server was stopped.".into())
        );
        assert!(f.core.session_mut(&f.cluster).unwrap().forward.is_none());
    }

    #[test]
    fn signing_in_needs_a_known_context_and_a_server_service() {
        let mut f = fixture();
        let nowhere = ClusterId::new("nowhere");
        let mut answer = f.core.sign_in(
            &nowhere,
            install("argocd", "u1"),
            Credentials::Token(SecretString::from("t".to_string())),
            &mut f.host,
        );
        assert_eq!(
            answer.try_recv().unwrap().unwrap().unwrap_err(),
            "unknown context"
        );

        let mut without = install("argocd", "u1");
        without.server = None;
        let mut answer = f.core.sign_in(
            &f.cluster,
            without,
            Credentials::Token(SecretString::from("t".to_string())),
            &mut f.host,
        );
        assert_eq!(
            answer.try_recv().unwrap().unwrap().unwrap_err(),
            "No argocd-server Service found in argocd"
        );
    }

    #[test]
    fn crds_coming_and_going_change_the_tree_groups() {
        let mut f = fixture();
        let caps = ArgoCdCaps {
            applications: true,
            ..Default::default()
        };

        // No client yet: detection can't run, but the explorer's group appears.
        f.core.sync_cluster(
            &f.cluster,
            caps,
            true,
            Link {
                client: None,
                context: Some(f.context.clone()),
            },
            Vec::new(),
            &mut f.host,
        );
        assert!(matches!(
            f.host.effects.as_slice(),
            [ArgoEffect::TreeGroupsChanged]
        ));

        // Argo CD goes away: the group goes with it.
        f.core.sync_cluster(
            &f.cluster,
            ArgoCdCaps::default(),
            true,
            Link::default(),
            Vec::new(),
            &mut f.host,
        );
        assert_eq!(f.host.effects.len(), 2);
        assert_eq!(f.core.detection(&f.cluster), Detection::Idle);
    }

    #[test]
    fn detection_hints_are_the_controller_namespaces_applications_report() {
        let mut stores = MemoryStores::default();
        let cluster = ClusterId::new("c");
        let applications = |cluster: &ClusterId| {
            StoreKey::new(
                cluster.clone(),
                kubyl_base::Gvr::new(GROUP, "v1alpha1", "applications"),
                None,
            )
        };
        stores.set(
            applications(&cluster),
            [
                json!({"metadata": {"name": "a", "namespace": "x"},
                    "status": {"controllerNamespace": "argocd"}}),
                json!({"metadata": {"name": "b", "namespace": "x"},
                    "status": {"controllerNamespace": "argocd"}}),
                json!({"metadata": {"name": "c", "namespace": "x"},
                    "status": {"controllerNamespace": "gitops"}}),
            ],
        );
        // Another cluster's applications say nothing about this one.
        stores.set(
            applications(&ClusterId::new("other")),
            [json!({"metadata": {"name": "d", "namespace": "x"},
                "status": {"controllerNamespace": "elsewhere"}})],
        );

        let mut hints = controller_namespaces(&cluster, &stores);
        hints.sort();
        assert_eq!(hints, ["argocd", "gitops"]);
    }

    #[test]
    fn saved_passwords_round_trip() {
        let saved = SavedPassword {
            username: "admin".into(),
            password: SecretString::from("p\"ss\\word".to_string()),
        };
        let parsed = SavedPassword::parse(&saved.to_secret()).unwrap();
        assert_eq!(parsed.username, "admin");
        assert_eq!(parsed.password.expose_secret(), "p\"ss\\word");
        assert!(SavedPassword::parse(&SecretString::from("a-token".to_string())).is_none());
        assert_eq!(
            password_key("argocd/s/c/ns/svc"),
            "argocd/s/c/ns/svc/password"
        );
    }
}
