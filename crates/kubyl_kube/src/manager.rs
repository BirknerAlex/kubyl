//! [`ConnectionManager`]: the app-wide entity that owns kubeconfig sources, contexts and one
//! connection per cluster.

use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::Duration;

use futures::StreamExt as _;
use futures::channel::mpsc;
use gpui::{
    App, AppContext as _, AsyncApp, Context, Entity, EventEmitter, Global, Hsla, SharedString,
    Subscription, Task, WeakEntity,
};
use k8s_openapi::apimachinery::pkg::version::Info;
use kube::config::Kubeconfig;
use kubyl_core::{
    ActiveContext, ClusterBadge, ClusterCaps, ClusterId, Notification, NotificationCenter,
    spawn_kube,
};
use kubyl_settings::{Settings, State, StateSection};
use serde::{Deserialize, Serialize};

use crate::access::{self, AccessCache, AccessQuery};
use crate::auth::CredentialSource;
use crate::client::{self, BuiltClient, ConnectError, Probe};
use crate::cluster_info::{self, ClusterInfo};
use crate::discovery::{self, Discovery};
use crate::groups::{self, GroupOptions};
use crate::kubeconfig::{self, ContextInfo, Loaded, Source, SourceSpec};
use crate::openapi;
use crate::settings::{ContextSettings, KubeSettings, context_color, display_path};
use crate::watches::{self, NamespaceUpdate};

/// Delay before re-running discovery after CRDs change (installs come in bursts).
const CRD_DEBOUNCE: Duration = Duration::from_millis(800);
/// While a CRD isn't served yet (not Established), discovery re-runs: 1 s doubling, 5 times.
const UNSERVED_DELAY: Duration = Duration::from_secs(1);
const UNSERVED_RETRIES: u32 = 5;
/// Watch updates are applied at most this often.
const BATCH_INTERVAL: Duration = Duration::from_millis(100);
/// Reconnect backoff after failures: 5 s doubling up to 60 s.
const BACKOFF_START: Duration = Duration::from_secs(5);
const BACKOFF_MAX: Duration = Duration::from_secs(60);
/// File events are coalesced for this long before reloading.
const RELOAD_DEBOUNCE: Duration = Duration::from_millis(300);

/// The connection state of one context.
#[derive(Clone, Debug, PartialEq)]
pub enum ConnectionState {
    Disconnected,
    Connecting,
    Connected {
        latency: Duration,
        /// `v1.33.1`.
        version: String,
    },
    /// Credentials are missing or were rejected. `sign_in`: an OIDC sign-in fixes it.
    AuthRequired {
        message: String,
        /// Exec plugin stderr.
        detail: Option<String>,
        sign_in: bool,
    },
    Unreachable {
        message: String,
        /// When the next reconnect attempt happens.
        retry_in: Option<Duration>,
    },
    /// Authenticated, but the user may not even read the API.
    Forbidden(String),
}

impl ConnectionState {
    pub fn is_connected(&self) -> bool {
        matches!(self, ConnectionState::Connected { .. })
    }

    /// `Connected · 38ms`, `Sign-in required`, `Unreachable`…
    pub fn label(&self) -> String {
        match self {
            ConnectionState::Disconnected => "Not connected".into(),
            ConnectionState::Connecting => "Connecting…".into(),
            ConnectionState::Connected { latency, .. } => {
                format!("Connected · {}ms", latency.as_millis().max(1))
            }
            ConnectionState::AuthRequired { sign_in: true, .. } => "Sign-in required".into(),
            ConnectionState::AuthRequired { .. } => "Auth failed".into(),
            ConnectionState::Unreachable { .. } => "Unreachable".into(),
            ConnectionState::Forbidden(_) => "Forbidden".into(),
        }
    }

    /// The error to show, if any.
    pub fn error(&self) -> Option<&str> {
        match self {
            ConnectionState::AuthRequired { message, .. }
            | ConnectionState::Unreachable { message, .. }
            | ConnectionState::Forbidden(message) => Some(message),
            _ => None,
        }
    }

    /// Theme color for status dots.
    pub fn color(&self, colors: &kubyl_ui::Colors) -> Hsla {
        match self {
            ConnectionState::Disconnected => colors.text_faint,
            ConnectionState::Connecting => colors.yellow,
            ConnectionState::Connected { .. } => colors.green,
            ConnectionState::AuthRequired { .. } => colors.yellow,
            ConnectionState::Unreachable { .. } | ConnectionState::Forbidden(_) => colors.red,
        }
    }
}

/// The namespaces of a cluster.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Namespaces {
    /// Sorted names.
    pub names: Vec<String>,
    /// `true`: from a live watch. `false`: listing is forbidden (or pending) and the names come
    /// from the kubeconfig and the user's `namespaces` setting.
    pub listed: bool,
    /// The namespaces of the entry's kubeconfig contexts (a group has one per member), offered
    /// in the namespace picker as "from kubeconfig".
    pub kubeconfig: Vec<String>,
}

/// Everything known about one connected (or connecting) cluster.
pub struct Cluster {
    pub state: ConnectionState,
    pub info: Option<ClusterInfo>,
    pub caps: ClusterCaps,
    pub discovery: Option<Arc<Discovery>>,
    /// Number of CRDs, once the CRD watch has listed them.
    pub crd_count: Option<usize>,
    /// Their names (`<plural>.<group>`).
    crds: Vec<String>,
    /// Discovery re-runs so far for CRDs that aren't served yet.
    unserved_retries: u32,
    pub namespaces: Namespaces,
    /// The proxy in use.
    pub proxy: Option<String>,
    /// When the current token or client certificate expires, if known.
    pub credential_expires_at: Option<jiff::Timestamp>,
    client: Option<BuiltClient>,
    version: Option<Info>,
    access: AccessCache,
    /// Bumped on every (re)connect; results of older attempts are ignored.
    generation: u64,
    /// Watches, health loop and in-flight requests. Dropping them aborts them.
    tasks: Vec<Task<()>>,
    failures: u32,
}

impl Default for Cluster {
    fn default() -> Self {
        Self {
            state: ConnectionState::Disconnected,
            info: None,
            caps: ClusterCaps::default(),
            discovery: None,
            crd_count: None,
            crds: Vec::new(),
            unserved_retries: 0,
            namespaces: Namespaces::default(),
            proxy: None,
            credential_expires_at: None,
            client: None,
            version: None,
            access: AccessCache::default(),
            generation: 0,
            tasks: Vec::new(),
            failures: 0,
        }
    }
}

impl Cluster {
    /// The kube client, once connected.
    pub fn client(&self) -> Option<kube::Client> {
        self.client.as_ref().map(|c| c.client.clone())
    }

    pub fn default_namespace(&self) -> Option<&str> {
        self.client.as_ref().map(|c| c.default_namespace.as_str())
    }
}

/// What changed. Subscribe with `cx.subscribe(&ConnectionManager::global(cx), …)`.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum ConnectionEvent {
    /// Sources or contexts were (re)loaded, or per-context settings changed.
    ContextsChanged,
    StateChanged(ClusterId),
    NamespacesChanged(ClusterId),
    /// Discovery finished or re-ran (CRDs changed).
    DiscoveryChanged(ClusterId),
    /// The active cluster changed.
    ActiveChanged(Option<ClusterId>),
    /// The user asked for this cluster and it needs an OIDC sign-in; the UI opens the modal.
    SignInRequested(ClusterId),
    /// An entry's id changed (a context got its first sibling, or lost its last one): its
    /// connection moved to `to` without reconnecting. Things keyed by `from` should follow.
    Rekeyed {
        from: ClusterId,
        to: ClusterId,
    },
}

/// What Kubyl remembers in state.json. Never credentials.
#[derive(Clone, Debug, Default, Serialize, Deserialize)]
#[serde(default)]
struct KubeState {
    active: Option<ClusterId>,
    /// The namespace last used in each group (where a group starts after a restart when the
    /// file's current context isn't one of its members).
    #[serde(skip_serializing_if = "std::collections::BTreeMap::is_empty")]
    namespaces: std::collections::BTreeMap<String, String>,
}

impl StateSection for KubeState {
    const KEY: &'static str = "kube";
}

/// Owns kubeconfig sources, the merged context list and one connection per cluster.
///
/// There is one per app: [`ConnectionManager::global`]. See the crate docs for how the sidebar
/// and other crates use it.
pub struct ConnectionManager {
    pasted_dir: PathBuf,
    loaded: Loaded,
    /// The sidebar entries: groups of contexts and single contexts (see [`crate::groups`]).
    entries: Vec<ContextInfo>,
    /// Entry index by entry id, and by member context id.
    entry_index: HashMap<ClusterId, usize>,
    member_index: HashMap<ClusterId, usize>,
    /// Ids of entries that were re-keyed this session, to their current id.
    aliases: HashMap<ClusterId, ClusterId>,
    /// Hash of each context's kubeconfig entries, to reconnect when they change.
    fingerprints: HashMap<ClusterId, u64>,
    clusters: HashMap<ClusterId, Cluster>,
    active: Option<ClusterId>,
    restored: bool,
    loading: bool,
    watch_files: bool,
    watcher: Option<notify::RecommendedWatcher>,
    watched: Vec<PathBuf>,
    file_events: Option<mpsc::UnboundedSender<()>>,
    settings: KubeSettings,
    /// Clusters the user explicitly asked for; an OIDC sign-in prompt is shown for them.
    interactive: HashSet<ClusterId>,
    _tasks: Vec<Task<()>>,
    _subscriptions: Vec<Subscription>,
}

impl EventEmitter<ConnectionEvent> for ConnectionManager {}

struct GlobalManager(Entity<ConnectionManager>);

impl Global for GlobalManager {}

impl ConnectionManager {
    /// The app's manager.
    ///
    /// # Panics
    /// Before [`crate::init`].
    pub fn global(cx: &App) -> Entity<Self> {
        cx.global::<GlobalManager>().0.clone()
    }

    pub fn try_global(cx: &App) -> Option<Entity<Self>> {
        cx.try_global::<GlobalManager>().map(|g| g.0.clone())
    }

    pub(crate) fn install(pasted_dir: PathBuf, watch_files: bool, cx: &mut App) -> Entity<Self> {
        let manager = cx.new(|cx| Self::new(pasted_dir, watch_files, cx));
        cx.set_global(GlobalManager(manager.clone()));
        let weak = manager.downgrade();
        kubyl_core::ClusterIds::install(cx, move |id, cx| {
            weak.upgrade()
                .map(|m| m.read(cx).resolve(id))
                .unwrap_or_else(|| id.clone())
        });
        manager
    }

    fn new(pasted_dir: PathBuf, watch_files: bool, cx: &mut Context<Self>) -> Self {
        let weak = cx.weak_entity();
        let subscription = Settings::observe::<KubeSettings>(cx, move |_, cx| {
            weak.update(cx, |this, cx| this.settings_changed(cx)).ok();
        });
        let namespaces = cx.observe_global::<ActiveContext>(|this, cx| this.remember_namespace(cx));
        let mut this = Self {
            pasted_dir,
            loaded: Loaded::default(),
            entries: Vec::new(),
            entry_index: HashMap::new(),
            member_index: HashMap::new(),
            aliases: HashMap::new(),
            fingerprints: HashMap::new(),
            clusters: HashMap::new(),
            active: None,
            restored: false,
            loading: false,
            watch_files,
            watcher: None,
            watched: Vec::new(),
            file_events: None,
            settings: Settings::get::<KubeSettings>(cx).clone(),
            interactive: HashSet::new(),
            _tasks: Vec::new(),
            _subscriptions: vec![subscription, namespaces],
        };
        if watch_files {
            this.start_file_events(cx);
        }
        this.reload(cx);
        this
    }

    // ----- Sources and contexts -----

    /// Kubeconfig sources in display order.
    pub fn sources(&self) -> &[Source] {
        &self.loaded.sources
    }

    /// Every context of every kubeconfig, including hidden ones and members of groups.
    pub fn all_contexts(&self) -> &[ContextInfo] {
        &self.loaded.contexts
    }

    /// Every cluster entry (groups and single contexts), including hidden ones.
    pub fn entries(&self) -> &[ContextInfo] {
        &self.entries
    }

    /// The cluster entries the user didn't hide: one per cluster and user (a group of contexts
    /// that differ only in their namespace), or one per context with grouping off.
    pub fn contexts(&self) -> impl Iterator<Item = &ContextInfo> {
        self.entries
            .iter()
            .filter(|c| !self.context_settings(&c.id).hidden)
    }

    /// The entry of `id` (an entry id, a member's context id or an older id of the entry).
    pub fn context(&self, id: &ClusterId) -> Option<&ContextInfo> {
        let ix = self
            .entry_index
            .get(id)
            .or_else(|| self.entry_index.get(&self.resolve(id)))?;
        self.entries.get(*ix)
    }

    /// One context of a kubeconfig by its own id (`<context>@<file>`), group member or not.
    pub fn member(&self, id: &ClusterId) -> Option<&ContextInfo> {
        self.loaded.contexts.iter().find(|c| &c.id == id)
    }

    /// The current id of the entry `id` belongs to: `id` itself for an entry, the group of a
    /// member context, the new id of a re-keyed entry, or for a group id that isn't one anymore
    /// (grouping turned off, siblings removed) the context that took its place. Everything that
    /// reads persisted cluster ids goes through this.
    pub fn resolve(&self, id: &ClusterId) -> ClusterId {
        resolve_in(
            id,
            &self.entries,
            &self.entry_index,
            &self.member_index,
            &self.aliases,
            &self.loaded,
        )
    }

    /// Keys that per-cluster settings of other crates may use for this entry: its id, its
    /// members' ids and its members' context names (`metrics.prometheus`, `alerts.clusters`).
    pub fn settings_keys(&self, id: &ClusterId) -> Vec<String> {
        let Some(entry) = self.context(id) else {
            return vec![id.to_string()];
        };
        let mut keys = vec![entry.id.to_string()];
        for member in &entry.members {
            keys.push(member.id.to_string());
        }
        for member in &entry.members {
            keys.push(member.context.clone());
        }
        keys.dedup();
        let mut seen = HashSet::new();
        keys.retain(|k| seen.insert(k.clone()));
        keys
    }

    /// The user's overrides for an entry: its own, merged with its members' (see
    /// [`KubeSettings::merged`]).
    pub fn context_settings(&self, id: &ClusterId) -> ContextSettings {
        match self.context(id) {
            Some(entry) => {
                let mut keys = vec![entry.id.as_str()];
                if entry.is_group() {
                    keys.extend(entry.members.iter().map(|m| m.id.as_str()));
                }
                let mut merged = self.settings.merged(&keys);
                // A context of a group shown separately keeps the group's safety flags.
                if let Some(group) = separated_group(entry) {
                    let group = self.settings.context(group.as_str());
                    merged.production |= group.production;
                    merged.read_only |= group.read_only;
                }
                merged
            }
            None => self.settings.context(id.as_str()),
        }
    }

    /// Display name: the override, or the entry's label.
    pub fn display_name(&self, id: &ClusterId) -> SharedString {
        self.context_settings(id)
            .display_name
            .map(SharedString::from)
            .or_else(|| self.context(id).map(|c| SharedString::from(c.name.clone())))
            .unwrap_or_else(|| SharedString::from(id.to_string()))
    }

    pub fn color(&self, id: &ClusterId, cx: &App) -> Hsla {
        let id = self.resolve(id);
        context_color(id.as_str(), &self.context_settings(&id), cx)
    }

    /// True until the first load finished.
    pub fn is_loading(&self) -> bool {
        self.loading && self.loaded.sources.is_empty()
    }

    /// Where pasted kubeconfigs are saved.
    pub fn pasted_dir(&self) -> &Path {
        &self.pasted_dir
    }

    /// Re-reads every source.
    pub fn reload(&mut self, cx: &mut Context<Self>) {
        let specs = kubeconfig::source_specs(
            &self.settings,
            std::env::var_os("KUBECONFIG"),
            dirs::home_dir(),
            &self.pasted_dir,
        );
        self.loading = true;
        let load = cx
            .background_executor()
            .spawn(async move { (kubeconfig::load(&specs), specs) });
        self._tasks.push(cx.spawn(async move |this, cx| {
            let (loaded, specs) = load.await;
            this.update(cx, |this, cx| this.apply_loaded(loaded, &specs, cx))
                .ok();
        }));
        // Finished loads stay in the list until the next reload; keep it short.
        if self._tasks.len() > 8 {
            self._tasks.drain(..self._tasks.len() - 4);
        }
    }

    fn apply_loaded(&mut self, loaded: Loaded, specs: &[SourceSpec], cx: &mut Context<Self>) {
        self.loading = false;
        tracing::info!(
            sources = loaded.sources.len(),
            contexts = loaded.contexts.len(),
            "kubeconfigs loaded"
        );
        self.loaded = loaded;
        self.apply_entries(cx);
        if self.watch_files {
            self.update_watcher(specs);
        }
        if !self.restored {
            self.restored = true;
            if let Some(id) = State::get::<KubeState>(cx).active {
                let id = self.resolve(&id);
                if self.context(&id).is_some() {
                    self.activate(&id, cx);
                }
            }
        }
        self.sync_active(cx);
        cx.emit(ConnectionEvent::ContextsChanged);
        cx.notify();
    }

    /// Rebuilds the entries from the loaded contexts and the grouping settings. Connections of
    /// entries whose id changed (a context got its first sibling, or lost its last one) move to
    /// the new id without reconnecting; removed ones are dropped; changed ones reconnect. A new
    /// namespace or current context changes nothing about a connection.
    fn apply_entries(&mut self, cx: &mut Context<Self>) {
        let options = GroupOptions {
            enabled: self.settings.group_contexts,
            separate: self.settings.separate_groups.clone(),
        };
        let entries = groups::entries(&self.loaded.contexts, &self.loaded.configs, &options);
        let (entry_index, member_index) = index(&entries);
        let fingerprints: HashMap<ClusterId, u64> = entries
            .iter()
            .map(|e| {
                (
                    e.id.clone(),
                    fingerprint(e, self.loaded.configs.get(&e.file)),
                )
            })
            .collect();

        let gone: Vec<ClusterId> = self
            .clusters
            .keys()
            .filter(|id| !entry_index.contains_key(*id))
            .cloned()
            .collect();
        let mut rekeyed = Vec::new();
        let mut removed = Vec::new();
        for old in gone {
            let to = resolve_in(
                &old,
                &entries,
                &entry_index,
                &member_index,
                &self.aliases,
                &self.loaded,
            );
            let same_connection = entry_index.contains_key(&to)
                && !self.clusters.contains_key(&to)
                && self.fingerprints.contains_key(&old)
                && self.fingerprints.get(&old) == fingerprints.get(&to);
            match self.clusters.remove(&old) {
                Some(cluster) if same_connection => {
                    self.clusters.insert(to.clone(), cluster);
                    rekeyed.push((old, to));
                }
                _ => removed.push(old),
            }
        }
        let changed: Vec<ClusterId> = self
            .clusters
            .iter()
            .filter(|(id, cluster)| {
                cluster.state != ConnectionState::Disconnected
                    && !rekeyed.iter().any(|(_, to)| to == *id)
                    && self.fingerprints.get(*id) != fingerprints.get(*id)
            })
            .map(|(id, _)| id.clone())
            .collect();
        self.entries = entries;
        self.entry_index = entry_index;
        self.member_index = member_index;
        self.fingerprints = fingerprints;

        for (from, to) in &rekeyed {
            tracing::info!(%from, %to, "cluster entry re-keyed; the connection stays");
            for target in self.aliases.values_mut() {
                if target == from {
                    *target = to.clone();
                }
            }
            self.aliases.insert(from.clone(), to.clone());
            if self.interactive.remove(from) {
                self.interactive.insert(to.clone());
            }
            if self.active.as_ref() == Some(from) {
                self.active = Some(to.clone());
                State::update::<KubeState>(cx, |state| state.active = Some(to.clone()));
            }
            cx.emit(ConnectionEvent::Rekeyed {
                from: from.clone(),
                to: to.clone(),
            });
            cx.emit(ConnectionEvent::StateChanged(to.clone()));
        }
        for id in removed {
            cx.emit(ConnectionEvent::StateChanged(id));
        }
        for id in changed {
            tracing::info!(context = %id, "kubeconfig changed; reconnecting");
            self.connect(&id, cx);
        }
        // Members and their namespaces may have changed.
        let ids: Vec<ClusterId> = self.clusters.keys().cloned().collect();
        for id in ids {
            self.refresh_fallback_namespaces(&id, cx);
        }
        if self
            .active
            .as_ref()
            .is_some_and(|id| self.context(id).is_none())
        {
            self.active = None;
            ActiveContext::set(cx, ActiveContext::default());
            cx.emit(ConnectionEvent::ActiveChanged(None));
        }
        // Tabs with ids that resolve differently now are rebuilt by the workspace.
        kubyl_core::ClusterIds::changed(cx);
    }

    /// The namespaces of an entry whose listing is forbidden, after its settings or members
    /// changed.
    fn refresh_fallback_namespaces(&mut self, id: &ClusterId, cx: &mut Context<Self>) {
        let settings = self.context_settings(id);
        let info = self.context(id).cloned();
        let key = self.key(id);
        let Some(cluster) = self.clusters.get_mut(&key) else {
            return;
        };
        let kubeconfig = kubeconfig_namespaces(info.as_ref());
        if cluster.namespaces.listed {
            if cluster.namespaces.kubeconfig != kubeconfig {
                cluster.namespaces.kubeconfig = kubeconfig;
                cx.emit(ConnectionEvent::NamespacesChanged(id.clone()));
            }
            return;
        }
        let namespaces = fallback_namespaces(info.as_ref(), &settings, cluster.default_namespace());
        if cluster.namespaces != namespaces {
            cluster.namespaces = namespaces;
            cx.emit(ConnectionEvent::NamespacesChanged(id.clone()));
        }
    }

    fn settings_changed(&mut self, cx: &mut Context<Self>) {
        let new = Settings::get::<KubeSettings>(cx).clone();
        let old = std::mem::replace(&mut self.settings, new.clone());
        if old.kubeconfigs != new.kubeconfigs
            || old.load_default_kubeconfig != new.load_default_kubeconfig
            || old.load_kubeconfig_env != new.load_kubeconfig_env
        {
            self.reload(cx);
        }
        let regroup =
            old.group_contexts != new.group_contexts || old.separate_groups != new.separate_groups;
        if regroup {
            self.apply_entries(cx);
        }
        if old.contexts != new.contexts || regroup {
            let ids: Vec<ClusterId> = self.clusters.keys().cloned().collect();
            for id in ids {
                self.refresh_caps(&id);
                self.refresh_fallback_namespaces(&id, cx);
            }
            self.sync_active(cx);
            cx.emit(ConnectionEvent::ContextsChanged);
        }
        cx.notify();
    }

    /// Adds kubeconfig files or folders as sources (stored in settings.json).
    pub fn add_sources(&mut self, paths: Vec<PathBuf>, cx: &mut Context<Self>) {
        let existing: HashSet<PathBuf> = self
            .loaded
            .sources
            .iter()
            .flat_map(|s| std::iter::once(s.spec.path.clone()).chain(s.spec.files.iter().cloned()))
            .collect();
        let new: Vec<String> = paths
            .into_iter()
            .filter(|p| !existing.contains(p))
            .map(|p| p.display().to_string())
            .collect();
        if new.is_empty() {
            NotificationCenter::push(
                cx,
                Notification::info("Those kubeconfigs are already loaded."),
            );
            return;
        }
        Settings::update::<KubeSettings>(cx, |settings| {
            for path in new {
                if !settings.kubeconfigs.contains(&path) {
                    settings.kubeconfigs.push(path);
                }
            }
        });
    }

    /// Removes a user-added source. Files are never deleted, except pasted ones.
    pub fn remove_source(&mut self, path: &Path, cx: &mut Context<Self>) {
        let display = display_path(path);
        let path_str = path.display().to_string();
        Settings::update::<KubeSettings>(cx, |settings| {
            settings.kubeconfigs.retain(|p| {
                p != &path_str && p != &display && crate::settings::expand_home(p) != path
            });
        });
    }

    /// Turns loading `~/.kube/config` on or off (settings.json). The file itself is untouched.
    pub fn set_load_default_kubeconfig(&mut self, load: bool, cx: &mut Context<Self>) {
        Settings::update::<KubeSettings>(cx, |settings| settings.load_default_kubeconfig = load);
    }

    /// Turns loading the files in `$KUBECONFIG` on or off (settings.json).
    pub fn set_load_kubeconfig_env(&mut self, load: bool, cx: &mut Context<Self>) {
        Settings::update::<KubeSettings>(cx, |settings| settings.load_kubeconfig_env = load);
    }

    /// Deletes a pasted kubeconfig: the copy Kubyl keeps in [`Self::pasted_dir`]. Refuses any
    /// other path, since Kubyl never deletes the user's own kubeconfig files.
    pub fn delete_pasted(
        &mut self,
        path: &Path,
        cx: &mut Context<Self>,
    ) -> Task<Result<(), String>> {
        if path.parent() != Some(self.pasted_dir.as_path()) {
            return Task::ready(Err(format!(
                "{} isn't a pasted kubeconfig",
                display_path(path)
            )));
        }
        let path = path.to_path_buf();
        let delete = cx
            .background_executor()
            .spawn(async move { std::fs::remove_file(&path).map_err(|e| e.to_string()) });
        cx.spawn(async move |this, cx| {
            delete.await?;
            this.update(cx, |this, cx| this.reload(cx)).ok();
            Ok(())
        })
    }

    /// Validates pasted kubeconfig YAML and saves it under the pasted kubeconfigs folder (0600).
    pub fn paste_kubeconfig(
        &mut self,
        name: String,
        yaml: String,
        cx: &mut Context<Self>,
    ) -> Task<Result<PathBuf, String>> {
        let dir = self.pasted_dir.clone();
        let save = cx.background_executor().spawn(async move {
            kubeconfig::validate_yaml(&yaml)?;
            kubeconfig::save_pasted(&dir, &name, &yaml).map_err(|e| e.to_string())
        });
        cx.spawn(async move |this, cx| {
            let path = save.await?;
            this.update(cx, |this, cx| this.reload(cx)).ok();
            Ok(path)
        })
    }

    /// Changes an entry's overrides in settings.json. A group's are written under the group
    /// id, starting from the merged settings; a safety flag turned off on a group is cleared on
    /// its members too (otherwise any member would keep it on).
    pub fn update_context_settings(
        &mut self,
        id: &ClusterId,
        cx: &mut Context<Self>,
        f: impl FnOnce(&mut ContextSettings),
    ) {
        let entry = self.context(id).cloned();
        // A context of a group shown separately: the group's safety flags apply to it (see
        // `context_settings`), so turning one off clears the group's, and its other contexts
        // keep the flag as their own.
        let (group, siblings) = match entry.as_ref().and_then(separated_group) {
            Some(group) => (
                Some(group.to_string()),
                self.entries
                    .iter()
                    .filter(|e| {
                        e.group.as_ref() == Some(group)
                            && Some(&e.id) != entry.as_ref().map(|x| &x.id)
                    })
                    .map(|e| e.id.to_string())
                    .collect::<Vec<_>>(),
            ),
            None => (None, Vec::new()),
        };
        let (key, mut members, mut settings) = match &entry {
            Some(entry) if entry.is_group() => (
                entry.id.to_string(),
                entry.members.iter().map(|m| m.id.to_string()).collect(),
                self.context_settings(&entry.id),
            ),
            Some(entry) => (
                entry.id.to_string(),
                Vec::new(),
                self.context_settings(&entry.id),
            ),
            None => (
                id.to_string(),
                Vec::new(),
                self.settings.context(id.as_str()),
            ),
        };
        members.extend(group.clone());
        f(&mut settings);
        Settings::update::<KubeSettings>(cx, move |stored| {
            if let Some(group_settings) =
                group.as_ref().and_then(|g| stored.contexts.get(g)).cloned()
            {
                for sibling in &siblings {
                    let own = stored.contexts.entry(sibling.clone()).or_default();
                    own.production |= group_settings.production;
                    own.read_only |= group_settings.read_only;
                    if *own == ContextSettings::default() {
                        stored.contexts.remove(sibling);
                    }
                }
            }
            for member in &members {
                let Some(member_settings) = stored.contexts.get_mut(member) else {
                    continue;
                };
                member_settings.production &= settings.production;
                member_settings.read_only &= settings.read_only;
                member_settings.hidden &= settings.hidden;
                if *member_settings == ContextSettings::default() {
                    stored.contexts.remove(member);
                }
            }
            if settings == ContextSettings::default() {
                stored.contexts.remove(&key);
            } else {
                stored.contexts.insert(key, settings);
            }
        });
    }

    /// Turns grouping of contexts on or off (settings.json `kubernetes.group_contexts`).
    pub fn set_group_contexts(&mut self, group: bool, cx: &mut Context<Self>) {
        Settings::update::<KubeSettings>(cx, |settings| settings.group_contexts = group);
    }

    /// Shows a group's contexts as separate entries, or (`separate: false`) as one again.
    pub fn set_group_separate(
        &mut self,
        group: &ClusterId,
        separate: bool,
        cx: &mut Context<Self>,
    ) {
        let group = group.to_string();
        Settings::update::<KubeSettings>(cx, move |settings| {
            settings.separate_groups.retain(|g| g != &group);
            if separate {
                settings.separate_groups.push(group);
            }
        });
    }

    /// Moves a context's overrides to a new id (the kubeconfig editor renamed the context or
    /// moved it to another file). Existing overrides of `to` are replaced.
    pub fn move_context_settings(
        &mut self,
        from: &ClusterId,
        to: &ClusterId,
        cx: &mut Context<Self>,
    ) {
        let (from, to) = (from.to_string(), to.to_string());
        if from == to {
            return;
        }
        Settings::update::<KubeSettings>(cx, move |settings| {
            if let Some(entry) = settings.contexts.remove(&from) {
                settings.contexts.insert(to, entry);
            }
        });
    }

    // ----- File watching -----

    fn start_file_events(&mut self, cx: &mut Context<Self>) {
        let (tx, mut rx) = mpsc::unbounded::<()>();
        self.file_events = Some(tx);
        self._tasks.push(
            cx.spawn(async move |this: WeakEntity<Self>, cx: &mut AsyncApp| {
                while rx.next().await.is_some() {
                    cx.background_executor().timer(RELOAD_DEBOUNCE).await;
                    while rx.try_recv().is_ok() {}
                    if this.update(cx, |this, cx| this.reload(cx)).is_err() {
                        break;
                    }
                }
            }),
        );
    }

    /// Watches the parent folders of source files (editors replace files by renaming, which
    /// ends a watch on the file itself) and folder sources.
    fn update_watcher(&mut self, specs: &[SourceSpec]) {
        use notify::Watcher as _;
        let mut dirs: Vec<PathBuf> = Vec::new();
        let mut relevant: HashSet<PathBuf> = HashSet::new();
        // Events carry canonical paths (on macOS `/tmp/x` is reported as `/private/tmp/x`).
        let mut relevant_path = |path: &Path| {
            relevant.insert(path.to_path_buf());
            if let Ok(canonical) = std::fs::canonicalize(path) {
                relevant.insert(canonical);
            } else if let (Some(parent), Some(name)) = (path.parent(), path.file_name())
                && let Ok(parent) = std::fs::canonicalize(parent)
            {
                relevant.insert(parent.join(name));
            }
        };
        for spec in specs {
            if spec.is_dir {
                std::fs::create_dir_all(&spec.path).ok();
                relevant_path(&spec.path);
                dirs.push(spec.path.clone());
            } else {
                for file in &spec.files {
                    relevant_path(file);
                    if let Some(parent) = file.parent() {
                        dirs.push(parent.to_path_buf());
                    }
                }
            }
        }
        dirs.sort();
        dirs.dedup();
        dirs.retain(|d| d.is_dir());
        if dirs == self.watched && self.watcher.is_some() {
            return;
        }
        let Some(tx) = self.file_events.clone() else {
            return;
        };
        let watcher = notify::recommended_watcher(move |event: notify::Result<notify::Event>| {
            let Ok(event) = event else { return };
            if event.kind.is_access() {
                return;
            }
            let hit = event.paths.iter().any(|p| {
                relevant.contains(p) || p.parent().is_some_and(|parent| relevant.contains(parent))
            });
            if hit {
                tx.unbounded_send(()).ok();
            }
        });
        match watcher {
            Ok(mut watcher) => {
                for dir in &dirs {
                    if let Err(err) = watcher.watch(dir, notify::RecursiveMode::NonRecursive) {
                        tracing::warn!("not watching {}: {err}", dir.display());
                    }
                }
                self.watcher = Some(watcher);
                self.watched = dirs;
            }
            Err(err) => tracing::warn!("kubeconfig hot reload is off: {err}"),
        }
    }

    // ----- Connections -----

    /// The key of `id`'s connection: `id` itself, or the entry it resolves to (a member, or an
    /// id from before a re-key: tasks keep the id they were started with).
    fn key(&self, id: &ClusterId) -> ClusterId {
        if self.clusters.contains_key(id) {
            id.clone()
        } else {
            self.resolve(id)
        }
    }

    pub fn cluster(&self, id: &ClusterId) -> Option<&Cluster> {
        self.clusters.get(&self.key(id))
    }

    pub fn state(&self, id: &ClusterId) -> ConnectionState {
        self.clusters
            .get(&self.key(id))
            .map(|c| c.state.clone())
            .unwrap_or(ConnectionState::Disconnected)
    }

    /// The kube client of a connected cluster.
    pub fn client(&self, id: &ClusterId) -> Option<kube::Client> {
        self.clusters
            .get(&self.key(id))
            .filter(|c| c.state.is_connected())
            .and_then(Cluster::client)
    }

    pub fn discovery(&self, id: &ClusterId) -> Option<Arc<Discovery>> {
        self.clusters.get(&self.key(id))?.discovery.clone()
    }

    pub fn namespaces(&self, id: &ClusterId) -> Namespaces {
        self.clusters
            .get(&self.key(id))
            .map(|c| c.namespaces.clone())
            .unwrap_or_default()
    }

    /// Capabilities, including the user's production and read-only flags.
    pub fn caps(&self, id: &ClusterId) -> ClusterCaps {
        match self.clusters.get(&self.key(id)) {
            Some(cluster) => cluster.caps.clone(),
            None => cluster_info::caps(None, None, &self.context_settings(id)),
        }
    }

    /// Number of clusters currently connected.
    pub fn connected_count(&self) -> usize {
        self.clusters
            .values()
            .filter(|c| c.state.is_connected())
            .count()
    }

    pub fn active(&self) -> Option<&ClusterId> {
        self.active.as_ref()
    }

    /// Connects unless already connected or connecting. Use when a cluster root is expanded or
    /// a favorite is opened.
    pub fn ensure_connected(&mut self, id: &ClusterId, cx: &mut Context<Self>) {
        let id = &self.resolve(id);
        match self.state(id) {
            ConnectionState::Disconnected
            | ConnectionState::Unreachable { .. }
            | ConnectionState::Forbidden(_) => self.connect(id, cx),
            ConnectionState::AuthRequired { sign_in: false, .. } => self.connect(id, cx),
            _ => {}
        }
    }

    /// (Re)connects: builds a client, checks it, then starts discovery, watches and the health
    /// ping.
    pub fn connect(&mut self, id: &ClusterId, cx: &mut Context<Self>) {
        let id = &self.resolve(id);
        let Some(info) = self.context(id).cloned() else {
            return;
        };
        let cluster = self.clusters.entry(id.clone()).or_default();
        cluster.generation += 1;
        let generation = cluster.generation;
        // Aborts the old client's watches and pings.
        cluster.tasks = Vec::new();
        cluster.client = None;
        cluster.discovery = None;
        cluster.crd_count = None;
        cluster.crds.clear();
        cluster.unserved_retries = 0;
        cluster.access.clear();
        let config = self.loaded.configs.get(&info.file).cloned();
        let (Some(config), None) = (config, info.error.clone()) else {
            let message = info
                .error
                .clone()
                .unwrap_or_else(|| "kubeconfig not loaded".into());
            cluster.state = ConnectionState::Unreachable {
                message,
                retry_in: None,
            };
            cx.emit(ConnectionEvent::StateChanged(id.clone()));
            self.sync_active(cx);
            cx.notify();
            return;
        };
        cluster.state = ConnectionState::Connecting;

        let task_info = info.clone();
        let connect = spawn_kube(cx, async move {
            let built = client::build(&task_info, config).await?;
            let probe = match client::probe(&built.client).await {
                // A cached exec token may have been revoked; run the plugin once more.
                Err(ConnectError::Auth { .. })
                    if matches!(built.credentials, Some(CredentialSource::Exec(_))) =>
                {
                    if let Some(CredentialSource::Exec(exec)) = &built.credentials {
                        exec.invalidate().await;
                    }
                    client::probe(&built.client).await
                }
                other => other,
            };
            // A rejected OpenShift token: try the next one (Kubyl's sign-in, then the
            // kubeconfig's), then ask for a sign-in.
            let probe = match &built.credentials {
                Some(CredentialSource::OpenShift(auth)) => {
                    let mut probe = probe;
                    for _ in 0..2 {
                        if !matches!(probe, Err(ConnectError::Auth { .. })) {
                            break;
                        }
                        probe = if auth.invalidate().await {
                            client::probe(&built.client).await
                        } else {
                            Err(ConnectError::SignInRequired)
                        };
                    }
                    probe
                }
                _ => probe,
            };
            let expires_at = match &built.credentials {
                Some(source) => source.expires_at().await,
                None => built.rebuild_at,
            };
            Ok::<_, ConnectError>((built, probe, expires_at))
        });
        let id_task = id.clone();
        cluster.tasks.push(cx.spawn(async move |this, cx| {
            let result = connect.await;
            this.update(cx, |this, cx| {
                this.finish_connect(&id_task, generation, result, cx)
            })
            .ok();
        }));
        cx.emit(ConnectionEvent::StateChanged(id.clone()));
        self.sync_active(cx);
        cx.notify();
    }

    /// Disconnects and forgets the client (watches stop).
    pub fn disconnect(&mut self, id: &ClusterId, cx: &mut Context<Self>) {
        let id = &self.key(id);
        if let Some(cluster) = self.clusters.get_mut(&self.key(id)) {
            cluster.generation += 1;
            *cluster = Cluster {
                generation: cluster.generation,
                ..Default::default()
            };
            cx.emit(ConnectionEvent::StateChanged(id.clone()));
            self.sync_active(cx);
            cx.notify();
        }
    }

    #[allow(clippy::type_complexity)]
    fn finish_connect(
        &mut self,
        id: &ClusterId,
        generation: u64,
        result: Result<
            (
                BuiltClient,
                Result<Probe, ConnectError>,
                Option<jiff::Timestamp>,
            ),
            ConnectError,
        >,
        cx: &mut Context<Self>,
    ) {
        let id = &self.key(id);
        let Some(info) = self.context(id).cloned() else {
            return;
        };
        let settings = self.context_settings(id);
        let Some(cluster) = self
            .clusters
            .get_mut(&self.key(id))
            .filter(|c| c.generation == generation)
        else {
            return;
        };
        let (built, probe, expires_at) = match result {
            Ok((built, Ok(probe), expires_at)) => (built, probe, expires_at),
            Ok((built, Err(err), _)) => {
                // Keep the client: an OIDC sign-in reuses its credential source.
                cluster.client = Some(built);
                self.fail(id, generation, err, cx);
                return;
            }
            Err(err) => {
                self.fail(id, generation, err, cx);
                return;
            }
        };
        tracing::info!(
            context = %info.name,
            version = %probe.version.git_version,
            latency_ms = probe.latency.as_millis() as u64,
            "connected"
        );
        self.interactive.remove(id);
        let Some(cluster) = self.clusters.get_mut(&self.key(id)) else {
            return;
        };
        cluster.failures = 0;
        cluster.state = ConnectionState::Connected {
            latency: probe.latency,
            version: probe.version.git_version.clone(),
        };
        cluster.info = Some(ClusterInfo::new(&probe.version, &info, probe.user.clone()));
        cluster.version = Some(probe.version);
        cluster.proxy = built.proxy.clone();
        cluster.credential_expires_at = expires_at;
        cluster.namespaces =
            fallback_namespaces(Some(&info), &settings, Some(&built.default_namespace));
        let kube_client = built.client.clone();
        cluster.client = Some(built);
        cluster.caps = cluster_info::caps(None, cluster.info.as_ref(), &settings);

        self.start_watches(id, generation, kube_client.clone(), cx);
        self.run_discovery(id, generation, kube_client, cx);
        self.start_health(id, generation, cx);
        cx.emit(ConnectionEvent::StateChanged(id.clone()));
        cx.emit(ConnectionEvent::NamespacesChanged(id.clone()));
        self.sync_active(cx);
        cx.notify();
    }

    fn fail(&mut self, id: &ClusterId, generation: u64, err: ConnectError, cx: &mut Context<Self>) {
        let id = &self.key(id);
        let sign_in = self.context(id).is_some_and(|c| c.auth.supports_sign_in());
        let name = self.display_name(id);
        let Some(cluster) = self
            .clusters
            .get_mut(&self.key(id))
            .filter(|c| c.generation == generation)
        else {
            return;
        };
        cluster.failures += 1;
        let was_connected = cluster.state.is_connected();
        cluster.state = match &err {
            ConnectError::SignInRequired => ConnectionState::AuthRequired {
                message: "Sign in to continue.".into(),
                detail: None,
                sign_in: true,
            },
            ConnectError::Auth { message, detail } => ConnectionState::AuthRequired {
                message: message.clone(),
                detail: detail.clone(),
                sign_in,
            },
            ConnectError::Forbidden(message) => ConnectionState::Forbidden(message.clone()),
            ConnectError::Unreachable(message) | ConnectError::Config(message) => {
                let retry =
                    matches!(err, ConnectError::Unreachable(_)).then(|| backoff(cluster.failures));
                ConnectionState::Unreachable {
                    message: message.clone(),
                    retry_in: retry,
                }
            }
        };
        tracing::info!(context = %name, "connection failed: {err}");
        if matches!(err, ConnectError::SignInRequired) && self.interactive.remove(id) {
            cx.emit(ConnectionEvent::SignInRequested(id.clone()));
        }
        let Some(cluster) = self.clusters.get_mut(&self.key(id)) else {
            return;
        };
        // Only the first failure is worth a toast; the retries show in the UI.
        if cluster.failures == 1 && !matches!(err, ConnectError::SignInRequired) {
            let mut message = format!("{name}: {err}");
            if let ConnectError::Auth {
                detail: Some(detail),
                ..
            } = &err
            {
                message.push_str(&format!("\n{}", truncate(detail, 400)));
            }
            NotificationCenter::push(
                cx,
                Notification::error(message).title(if was_connected {
                    "Connection lost"
                } else {
                    "Couldn't connect"
                }),
            );
        }
        if let ConnectionState::Unreachable {
            retry_in: Some(delay),
            ..
        } = cluster.state
        {
            let id = id.clone();
            cluster.tasks.push(cx.spawn(async move |this, cx| {
                cx.background_executor().timer(delay).await;
                this.update(cx, |this, cx| {
                    let current = this.clusters.get(&this.key(&id)).map(|c| c.generation);
                    if current == Some(generation) {
                        // Replaces (and so cancels) this task; nothing runs after it.
                        this.connect(&id, cx);
                    }
                })
                .ok();
            }));
        }
        cx.emit(ConnectionEvent::StateChanged(id.clone()));
        self.sync_active(cx);
        cx.notify();
    }

    fn start_health(&mut self, id: &ClusterId, generation: u64, cx: &mut Context<Self>) {
        let interval = Duration::from_secs(self.settings.health_check_interval.max(5));
        let task_id = id.clone();
        let task = cx.spawn(async move |this, cx| {
            let id = task_id;
            let mut delay = interval;
            loop {
                cx.background_executor().timer(delay).await;
                let Ok(Some((client, rebuild_at))) = this.read_with(cx, |this, _| {
                    let cluster = this
                        .clusters
                        .get(&this.key(&id))
                        .filter(|c| c.generation == generation)?;
                    let built = cluster.client.as_ref()?;
                    Some((built.client.clone(), built.rebuild_at))
                }) else {
                    break;
                };
                if rebuild_at
                    .is_some_and(|at| jiff::Timestamp::now() + Duration::from_secs(60) >= at)
                {
                    // An exec client certificate is about to expire.
                    this.update(cx, |this, cx| this.connect(&id, cx)).ok();
                    break;
                }
                let ping = cx
                    .update(|cx| spawn_kube(cx, async move { client::ping(&client).await }))
                    .await;
                let Ok(ok) =
                    this.update(cx, |this, cx| this.finish_ping(&id, generation, ping, cx))
                else {
                    break;
                };
                delay = if ok {
                    interval
                } else {
                    let failures = this
                        .read_with(cx, |this, _| {
                            this.clusters.get(&this.key(&id)).map(|c| c.failures)
                        })
                        .ok()
                        .flatten()
                        .unwrap_or(1);
                    backoff(failures)
                };
            }
        });
        self.push_task(id, generation, task);
    }

    /// Keeps `task` alive as long as this connection attempt is current.
    fn push_task(&mut self, id: &ClusterId, generation: u64, task: Task<()>) {
        if let Some(cluster) = self
            .clusters
            .get_mut(&self.key(id))
            .filter(|c| c.generation == generation)
        {
            cluster.tasks.push(task);
        }
    }

    /// Returns whether the cluster is healthy.
    fn finish_ping(
        &mut self,
        id: &ClusterId,
        generation: u64,
        ping: Result<Duration, ConnectError>,
        cx: &mut Context<Self>,
    ) -> bool {
        let id = &self.key(id);
        let Some(cluster) = self
            .clusters
            .get_mut(&self.key(id))
            .filter(|c| c.generation == generation)
        else {
            return false;
        };
        match ping {
            Ok(latency) => {
                let version = match &cluster.state {
                    ConnectionState::Connected { version, .. } => version.clone(),
                    _ => cluster
                        .version
                        .as_ref()
                        .map(|v| v.git_version.clone())
                        .unwrap_or_default(),
                };
                let recovered = !cluster.state.is_connected();
                cluster.failures = 0;
                cluster.state = ConnectionState::Connected { latency, version };
                if recovered {
                    let name = self.display_name(id);
                    NotificationCenter::push(
                        cx,
                        Notification::success(format!("{name} is reachable again")),
                    );
                }
                cx.emit(ConnectionEvent::StateChanged(id.clone()));
                self.sync_active(cx);
                cx.notify();
                true
            }
            Err(err) => {
                let sign_in = self.context(id).is_some_and(|c| c.auth.supports_sign_in());
                let name = self.display_name(id);
                let cluster = self.clusters.get_mut(&self.key(id)).expect("checked above");
                cluster.failures += 1;
                let first = cluster.failures == 1;
                cluster.state = match &err {
                    ConnectError::SignInRequired => ConnectionState::AuthRequired {
                        message: "The session expired. Sign in again.".into(),
                        detail: None,
                        sign_in: true,
                    },
                    ConnectError::Auth { message, detail } => ConnectionState::AuthRequired {
                        message: message.clone(),
                        detail: detail.clone(),
                        sign_in,
                    },
                    ConnectError::Forbidden(message) => ConnectionState::Forbidden(message.clone()),
                    ConnectError::Unreachable(message) | ConnectError::Config(message) => {
                        ConnectionState::Unreachable {
                            message: message.clone(),
                            retry_in: Some(backoff(cluster.failures)),
                        }
                    }
                };
                if first {
                    NotificationCenter::push(
                        cx,
                        Notification::warning(format!("{name}: {err}")).title("Connection lost"),
                    );
                }
                cx.emit(ConnectionEvent::StateChanged(id.clone()));
                self.sync_active(cx);
                cx.notify();
                false
            }
        }
    }

    fn start_watches(
        &mut self,
        id: &ClusterId,
        generation: u64,
        client: kube::Client,
        cx: &mut Context<Self>,
    ) {
        // Namespaces.
        let (tx, mut rx) = mpsc::unbounded::<NamespaceUpdate>();
        let watch = spawn_kube(cx, watches::watch_namespaces(client.clone(), tx));
        let id_ns = id.clone();
        let apply = cx.spawn(async move |this, cx| {
            while let Some(mut update) = rx.next().await {
                while let Ok(next) = rx.try_recv() {
                    update = next;
                }
                if this
                    .update(cx, |this, cx| {
                        this.apply_namespaces(&id_ns, generation, update, cx)
                    })
                    .is_err()
                {
                    break;
                }
                cx.background_executor().timer(BATCH_INTERVAL).await;
            }
        });

        // CRDs: every change after the first list re-runs discovery.
        let (crd_tx, mut crd_rx) = mpsc::unbounded::<Vec<String>>();
        let crd_watch = spawn_kube(cx, watches::watch_crds(client.clone(), crd_tx));
        let id_crd = id.clone();
        let crds = cx.spawn(async move |this, cx| {
            let mut first = true;
            while let Some(mut crds) = crd_rx.next().await {
                if !first {
                    cx.background_executor().timer(CRD_DEBOUNCE).await;
                }
                while let Ok(next) = crd_rx.try_recv() {
                    crds = next;
                }
                let rediscover = !first;
                first = false;
                let alive = this.update(cx, |this, cx| {
                    let id_crd = this.key(&id_crd);
                    let Some(cluster) = this
                        .clusters
                        .get_mut(&this.key(&id_crd))
                        .filter(|c| c.generation == generation)
                    else {
                        return false;
                    };
                    cluster.crd_count = Some(crds.len());
                    cluster.crds = crds;
                    if rediscover {
                        tracing::info!(context = %id_crd, "CRDs changed; re-running discovery");
                        cluster.unserved_retries = 0;
                        this.run_discovery(&id_crd, generation, client.clone(), cx);
                    } else {
                        this.rediscover_unserved(&id_crd, generation, client.clone(), cx);
                    }
                    cx.emit(ConnectionEvent::DiscoveryChanged(id_crd.clone()));
                    cx.notify();
                    true
                });
                if !matches!(alive, Ok(true)) {
                    break;
                }
            }
        });
        for task in [watch, apply, crd_watch, crds] {
            self.push_task(id, generation, task);
        }
    }

    fn apply_namespaces(
        &mut self,
        id: &ClusterId,
        generation: u64,
        update: NamespaceUpdate,
        cx: &mut Context<Self>,
    ) {
        let id = &self.key(id);
        let settings = self.context_settings(id);
        let info = self.context(id).cloned();
        let Some(cluster) = self
            .clusters
            .get_mut(&self.key(id))
            .filter(|c| c.generation == generation)
        else {
            return;
        };
        cluster.namespaces = match update {
            NamespaceUpdate::Names(names) => Namespaces {
                names,
                listed: true,
                kubeconfig: kubeconfig_namespaces(info.as_ref()),
            },
            NamespaceUpdate::Forbidden => {
                tracing::info!(context = %id, "listing namespaces is forbidden; using fallbacks");
                fallback_namespaces(info.as_ref(), &settings, cluster.default_namespace())
            }
        };
        cx.emit(ConnectionEvent::NamespacesChanged(id.clone()));
        cx.notify();
    }

    /// A new CRD is only served once it's Established, a status update the CRD watch doesn't
    /// report. While a CRD the watch listed isn't in discovery, discovery re-runs (a few times).
    fn rediscover_unserved(
        &mut self,
        id: &ClusterId,
        generation: u64,
        client: kube::Client,
        cx: &mut Context<Self>,
    ) {
        let id = &self.key(id);
        let Some(cluster) = self
            .clusters
            .get_mut(&self.key(id))
            .filter(|c| c.generation == generation)
        else {
            return;
        };
        let Some(discovery) = cluster.discovery.as_ref() else {
            return;
        };
        let unserved = cluster
            .crds
            .iter()
            .filter(|name| !discovery.serves_crd(name))
            .count();
        if unserved == 0 || cluster.unserved_retries >= UNSERVED_RETRIES {
            return;
        }
        let delay = UNSERVED_DELAY * 2u32.pow(cluster.unserved_retries);
        cluster.unserved_retries += 1;
        let id = id.clone();
        let task_id = id.clone();
        let task = cx.spawn(async move |this, cx| {
            cx.background_executor().timer(delay).await;
            this.update(cx, |this, cx| {
                if this
                    .clusters
                    .get(&this.key(&task_id))
                    .is_some_and(|c| c.generation == generation)
                {
                    tracing::debug!(context = %task_id, unserved, "CRDs not served yet; re-running discovery");
                    this.run_discovery(&task_id, generation, client, cx);
                }
            })
            .ok();
        });
        self.push_task(&id, generation, task);
    }

    fn run_discovery(
        &mut self,
        id: &ClusterId,
        generation: u64,
        client: kube::Client,
        cx: &mut Context<Self>,
    ) {
        let retry_client = client.clone();
        let discover = spawn_kube(cx, async move { discovery::discover(&client).await });
        let task_id = id.clone();
        let task = cx.spawn(async move |this, cx| {
            let id = task_id;
            let result = discover.await;
            this.update(cx, |this, cx| {
                let id = this.key(&id);
                let settings = this.context_settings(&id);
                let info = this.context(&id).cloned();
                let Some(cluster) = this
                    .clusters
                    .get_mut(&this.key(&id))
                    .filter(|c| c.generation == generation)
                else {
                    return;
                };
                match result {
                    Ok(discovery) => {
                        tracing::info!(
                            context = %id,
                            kinds = discovery.kind_count(),
                            aggregated = discovery.aggregated,
                            "discovery done"
                        );
                        if let (Some(cluster_info), Some(version), Some(context)) = (
                            cluster.info.as_mut(),
                            cluster.version.as_ref(),
                            info.as_ref(),
                        ) {
                            cluster_info.apply_discovery(version, context, &discovery);
                        }
                        cluster.caps =
                            cluster_info::caps(Some(&discovery), cluster.info.as_ref(), &settings);
                        cluster.discovery = Some(Arc::new(discovery));
                        cx.emit(ConnectionEvent::DiscoveryChanged(id.clone()));
                        this.sync_active(cx);
                        this.rediscover_unserved(&id, generation, retry_client, cx);
                        cx.notify();
                    }
                    Err(err) => tracing::warn!(context = %id, "discovery failed: {err}"),
                }
            })
            .ok();
        });
        self.push_task(id, generation, task);
    }

    fn refresh_caps(&mut self, id: &ClusterId) {
        let id = &self.key(id);
        let settings = self.context_settings(id);
        if let Some(cluster) = self.clusters.get_mut(&self.key(id)) {
            cluster.caps = cluster_info::caps(
                cluster.discovery.as_deref(),
                cluster.info.as_ref(),
                &settings,
            );
        }
    }

    // ----- RBAC and OpenAPI -----

    /// Whether the current user may do `query`, from the cache or the API server. `None` when
    /// the cluster isn't connected or the check failed.
    pub fn can_i(
        &mut self,
        id: &ClusterId,
        query: AccessQuery,
        cx: &mut Context<Self>,
    ) -> Task<Option<bool>> {
        if let Some(answer) = self.cached_can_i(id, &query) {
            return Task::ready(Some(answer));
        }
        let Some(client) = self.client(id) else {
            return Task::ready(None);
        };
        let generation = self.clusters.get(&self.key(id)).map(|c| c.generation);
        let check = spawn_kube(cx, access::check(client, query.clone()));
        let id = id.clone();
        cx.spawn(async move |this, cx| {
            let allowed = check
                .await
                .inspect_err(|err| tracing::debug!("access check failed: {err}"))
                .ok()?;
            this.update(cx, |this, _| {
                if let Some(cluster) = this
                    .clusters
                    .get_mut(&this.key(&id))
                    .filter(|c| Some(c.generation) == generation)
                {
                    cluster.access.insert(query, allowed);
                }
            })
            .ok();
            Some(allowed)
        })
    }

    /// A cached answer, without a request.
    pub fn cached_can_i(&self, id: &ClusterId, query: &AccessQuery) -> Option<bool> {
        self.clusters.get(&self.key(id))?.access.get(query)
    }

    /// The OpenAPI v3 spec of a group-version (`OpenApiIndex::key(group, version)`), from the
    /// on-disk cache when unchanged.
    pub fn openapi_spec(
        &self,
        id: &ClusterId,
        path: String,
        cx: &mut Context<Self>,
    ) -> Task<Result<serde_json::Value, String>> {
        let Some(client) = self.client(id) else {
            return Task::ready(Err("not connected".into()));
        };
        let server = self
            .context(id)
            .and_then(|c| c.server.clone())
            .unwrap_or_default();
        let dir = openapi::cache_dir(&format!("{id}|{server}"));
        let task = spawn_kube(cx, async move {
            openapi::spec(&client, &dir, &path)
                .await
                .map_err(|e| e.to_string())
        });
        cx.background_executor().spawn(task)
    }

    /// The credentials Kubyl manages for a context (exec, OIDC, OpenShift), for the sign-in modal.
    pub fn credentials(&self, id: &ClusterId) -> Option<CredentialSource> {
        if let Some(source) = self
            .clusters
            .get(&self.key(id))?
            .client
            .as_ref()?
            .credentials
            .clone()
        {
            return Some(source);
        }
        None
    }

    /// The user's bearer token for a connected cluster (kubeconfig token or token file, exec
    /// plugin, OIDC), or `None` for client-certificate users. For in-cluster services that
    /// authenticate the user themselves; never log or store it.
    pub fn bearer_token(&self, id: &ClusterId) -> Option<crate::auth::BearerToken> {
        self.clusters
            .get(&self.key(id))
            .filter(|c| c.state.is_connected())?
            .client
            .as_ref()?
            .bearer
            .clone()
    }

    /// The OIDC credentials of a context, shared with its client if one exists.
    pub fn oidc_auth(&self, id: &ClusterId) -> Option<Arc<crate::auth::OidcAuth>> {
        if let Some(CredentialSource::Oidc(auth)) = self.credentials(id) {
            return Some(auth);
        }
        let info = self.context(id)?;
        let config = self.loaded.configs.get(&info.file)?;
        client::oidc_auth(info, config).map(Arc::new)
    }

    /// The OpenShift OAuth credentials of a context, once a connect attempt built them.
    pub fn openshift_auth(&self, id: &ClusterId) -> Option<Arc<crate::auth::OpenShiftAuth>> {
        if let Some(CredentialSource::OpenShift(auth)) = self.credentials(id) {
            return Some(auth);
        }
        let info = self.context(id)?;
        crate::auth::OpenShiftAuth::find(info.server.as_deref()?, info.user.as_deref()?)
    }

    /// Connects on the user's behalf: a context that needs a sign-in opens the modal.
    pub fn connect_interactive(&mut self, id: &ClusterId, cx: &mut Context<Self>) {
        self.interactive.insert(id.clone());
        self.connect(id, cx);
    }

    // ----- Active cluster -----

    /// Makes `id` the active cluster (title bar, status bar) and connects it.
    pub fn activate(&mut self, id: &ClusterId, cx: &mut Context<Self>) {
        let id = &self.resolve(id);
        if self.context(id).is_none() {
            return;
        }
        let changed = self.active.as_ref() != Some(id);
        self.active = Some(id.clone());
        State::update::<KubeState>(cx, |state| state.active = Some(id.clone()));
        let namespace = self.initial_namespace(id, cx);
        let badge = self.badge(id, cx);
        ActiveContext::set(
            cx,
            ActiveContext {
                cluster: Some(badge),
                namespace: namespace.map(SharedString::from),
            },
        );
        if changed {
            cx.emit(ConnectionEvent::ActiveChanged(Some(id.clone())));
        }
        self.interactive.insert(id.clone());
        if let ConnectionState::AuthRequired { sign_in: true, .. } = self.state(id) {
            self.interactive.remove(id);
            cx.emit(ConnectionEvent::SignInRequested(id.clone()));
        }
        self.ensure_connected(id, cx);
        cx.notify();
    }

    /// Where an entry starts: its default namespace, else its kubeconfig namespace. A group
    /// starts in the file's current context's namespace when that context is a member (after
    /// `oc project foo` Kubyl opens `foo` next time), else in the last namespace used in Kubyl,
    /// else in its first member's.
    fn initial_namespace(&self, id: &ClusterId, cx: &App) -> Option<String> {
        if let Some(namespace) = self.context_settings(id).default_namespace {
            return Some(namespace);
        }
        let entry = self.context(id)?;
        if !entry.is_group() {
            return entry.namespace.clone();
        }
        let current = self
            .loaded
            .configs
            .get(&entry.file)
            .and_then(|c| c.current_context.as_deref());
        if let Some(member) = entry
            .members
            .iter()
            .find(|m| Some(m.context.as_str()) == current)
        {
            return member.namespace.clone();
        }
        if let Some(namespace) = State::get::<KubeState>(cx)
            .namespaces
            .get(entry.id.as_str())
        {
            return Some(namespace.clone());
        }
        entry
            .members
            .iter()
            .find(|m| m.context == entry.context)
            .and_then(|m| m.namespace.clone())
    }

    /// Keeps the last namespace used in each group (state.json).
    fn remember_namespace(&mut self, cx: &mut Context<Self>) {
        let active = ActiveContext::global(cx);
        let (Some(cluster), Some(namespace)) = (&active.cluster, &active.namespace) else {
            return;
        };
        let id = cluster.id.clone();
        let namespace = namespace.to_string();
        if !self.context(&id).is_some_and(ContextInfo::is_group) {
            return;
        }
        if State::get::<KubeState>(cx).namespaces.get(id.as_str()) == Some(&namespace) {
            return;
        }
        State::update::<KubeState>(cx, |state| {
            state.namespaces.insert(id.to_string(), namespace);
        });
    }

    /// How the chrome shows a cluster.
    pub fn badge(&self, id: &ClusterId, cx: &App) -> ClusterBadge {
        let settings = self.context_settings(id);
        let state = self.state(id);
        let meta = match self
            .clusters
            .get(&self.key(id))
            .and_then(|c| c.info.as_ref())
        {
            Some(info) if state.is_connected() => Some(info.summary()),
            _ => Some(state.label()),
        };
        ClusterBadge {
            id: id.clone(),
            name: self.display_name(id),
            color: self.color(id, cx),
            production: settings.production,
            meta: meta.map(SharedString::from),
            connected: state.is_connected(),
        }
    }

    /// Keeps the title/status bar badge in sync, preserving the selected namespace.
    fn sync_active(&self, cx: &mut Context<Self>) {
        let Some(id) = &self.active else {
            return;
        };
        let badge = self.badge(id, cx);
        let current = ActiveContext::global(cx).clone();
        if current.cluster.as_ref() != Some(&badge) {
            ActiveContext::set(
                cx,
                ActiveContext {
                    cluster: Some(badge),
                    namespace: current.namespace,
                },
            );
        }
    }
}

fn backoff(failures: u32) -> Duration {
    let factor = 2u32.saturating_pow(failures.saturating_sub(1).min(8));
    (BACKOFF_START * factor).min(BACKOFF_MAX)
}

fn truncate(text: &str, max: usize) -> String {
    if text.chars().count() <= max {
        text.to_string()
    } else {
        format!("{}…", text.chars().take(max).collect::<String>())
    }
}

/// The namespaces of an entry's kubeconfig contexts (every member of a group).
fn kubeconfig_namespaces(info: Option<&ContextInfo>) -> Vec<String> {
    let mut names: Vec<String> = info
        .map(|i| {
            i.members
                .iter()
                .filter_map(|m| m.namespace.clone())
                .chain(i.namespace.clone())
                .collect()
        })
        .unwrap_or_default();
    names.sort();
    names.dedup();
    names
}

/// Namespaces when listing isn't possible: the configured default, the kubeconfig namespaces
/// (every member's), the client's default and the user's list.
fn fallback_namespaces(
    info: Option<&ContextInfo>,
    settings: &ContextSettings,
    client_default: Option<&str>,
) -> Namespaces {
    let kubeconfig = kubeconfig_namespaces(info);
    let mut names: Vec<String> = settings
        .default_namespace
        .iter()
        .cloned()
        .chain(kubeconfig.iter().cloned())
        .chain(client_default.map(String::from))
        .chain(settings.namespaces.iter().cloned())
        .collect();
    names.sort();
    names.dedup();
    Namespaces {
        names,
        listed: false,
        kubeconfig,
    }
}

/// Entry index by entry id, and by member context id.
fn index(entries: &[ContextInfo]) -> (HashMap<ClusterId, usize>, HashMap<ClusterId, usize>) {
    let mut by_id = HashMap::new();
    let mut by_member = HashMap::new();
    for (ix, entry) in entries.iter().enumerate() {
        by_id.insert(entry.id.clone(), ix);
        for member in &entry.members {
            by_member.insert(member.id.clone(), ix);
        }
    }
    (by_id, by_member)
}

/// See [`ConnectionManager::resolve`].
fn resolve_in(
    id: &ClusterId,
    entries: &[ContextInfo],
    entry_index: &HashMap<ClusterId, usize>,
    member_index: &HashMap<ClusterId, usize>,
    aliases: &HashMap<ClusterId, ClusterId>,
    loaded: &Loaded,
) -> ClusterId {
    let mut id = id.clone();
    for _ in 0..4 {
        if entry_index.contains_key(&id) {
            return id;
        }
        if let Some(&ix) = member_index.get(&id) {
            return entries[ix].id.clone();
        }
        match aliases.get(&id) {
            Some(next) => id = next.clone(),
            None => break,
        }
    }
    // A group id that isn't an entry anymore: the entry of the contexts that use its cluster
    // and user entries in its file (the file's current context first, then by name).
    if let Some((cluster, user, file)) = groups::parse_group_id(id.as_str()) {
        let current = loaded
            .configs
            .get(&file)
            .and_then(|c| c.current_context.clone());
        let found = loaded
            .contexts
            .iter()
            .filter(|c| {
                c.file == file && c.cluster == cluster && c.user.as_deref().unwrap_or("") == user
            })
            .min_by_key(|c| (Some(&c.context) != current.as_ref(), c.context.clone()));
        if let Some(&ix) = found.and_then(|c| member_index.get(&c.id)) {
            return entries[ix].id.clone();
        }
    }
    id
}

/// Hash of what a connection is made of: the contents of the entry's cluster and user and
/// the context's other fields, without names and namespace (a new namespace, current context
/// or connecting member never reconnects).
fn fingerprint(info: &ContextInfo, config: Option<&Arc<Kubeconfig>>) -> u64 {
    use std::hash::{Hash as _, Hasher as _};
    let mut hasher = std::collections::hash_map::DefaultHasher::new();
    if let Some(config) = config {
        let context = config
            .contexts
            .iter()
            .find(|c| c.name == info.context)
            .and_then(|c| c.context.as_ref())
            .and_then(|c| serde_json::to_value(c).ok())
            .map(|mut value| {
                if let Some(map) = value.as_object_mut() {
                    map.remove("namespace");
                    map.remove("cluster");
                    map.remove("user");
                }
                value
            });
        let cluster = config
            .clusters
            .iter()
            .find(|c| c.name == info.cluster)
            .map(|c| &c.cluster);
        let user = config
            .auth_infos
            .iter()
            .find(|u| Some(&u.name) == info.user.as_ref())
            .map(|u| &u.auth_info);
        // Only hashed in memory, never stored or logged.
        serde_json::to_string(&(context, cluster, user))
            .unwrap_or_default()
            .hash(&mut hasher);
    }
    hasher.finish()
}

/// The group of a context shown separately (the group's id differs from the entry's).
fn separated_group(entry: &ContextInfo) -> Option<&ClusterId> {
    entry
        .group
        .as_ref()
        .filter(|g| !entry.is_group() && *g != &entry.id)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::kubeconfig::tests::{DEV, OTHER};
    use gpui::TestAppContext;

    fn setup(cx: &mut TestAppContext) -> (tempfile::TempDir, Entity<ConnectionManager>) {
        let dir = tempfile::tempdir().unwrap();
        let kube = dir.path().join("kube");
        std::fs::create_dir_all(&kube).unwrap();
        std::fs::write(kube.join("dev.yaml"), DEV).unwrap();
        std::fs::write(kube.join("other.yaml"), OTHER).unwrap();
        let settings = serde_json::json!({
            "kubernetes": {
                "load_default_kubeconfig": false,
                "load_kubeconfig_env": false,
                "kubeconfigs": [kube.join("dev.yaml"), kube.join("other.yaml")],
            }
        });
        std::fs::write(dir.path().join("settings.json"), settings.to_string()).unwrap();
        let manager = cx.update(|cx| {
            kubyl_core::init(cx);
            kubyl_settings::init_with_dir(cx, dir.path());
            kubyl_ui::init(cx);
            Settings::register::<KubeSettings>(cx);
            ConnectionManager::install(dir.path().join("pasted"), false, cx)
        });
        cx.run_until_parked();
        (dir, manager)
    }

    #[gpui::test]
    fn loads_merges_and_reloads_on_change(cx: &mut TestAppContext) {
        let (dir, manager) = setup(cx);
        manager.read_with(cx, |m, _| {
            let names: Vec<_> = m.contexts().map(|c| c.name.clone()).collect();
            assert_eq!(names, ["kind-dev", "prod", "prod@other", "broken"]);
            assert_eq!(m.sources().len(), 3);
        });

        // Editing a file on disk shows up after a reload (the watcher triggers it in the app).
        let edited = DEV.replace(
            "name: kind-dev\n    context:",
            "name: kind-renamed\n    context:",
        );
        std::fs::write(dir.path().join("kube/dev.yaml"), edited).unwrap();
        manager.update(cx, |m, cx| m.reload(cx));
        cx.run_until_parked();
        manager.read_with(cx, |m, _| {
            assert!(m.contexts().any(|c| c.name == "kind-renamed"));
            assert!(!m.contexts().any(|c| c.name == "kind-dev"));
        });
    }

    #[gpui::test]
    fn overrides_hide_contexts_and_drive_the_badge(cx: &mut TestAppContext) {
        let (_dir, manager) = setup(cx);
        let prod = manager.read_with(cx, |m, _| m.contexts().nth(1).unwrap().id.clone());
        let broken = manager.read_with(cx, |m, _| m.contexts().nth(3).unwrap().id.clone());
        manager.update(cx, |m, cx| {
            m.update_context_settings(&prod, cx, |s| {
                s.production = true;
                s.display_name = Some("Production".into());
            });
            m.update_context_settings(&broken, cx, |s| s.hidden = true);
        });
        cx.run_until_parked();
        manager.read_with(cx, |m, cx| {
            assert_eq!(m.contexts().count(), 3);
            assert_eq!(m.all_contexts().len(), 4);
            let badge = m.badge(&prod, cx);
            assert!(badge.production);
            assert_eq!(badge.name.as_ref(), "Production");
            assert!(!badge.connected);
            assert!(m.caps(&prod).production);
        });
    }

    #[gpui::test]
    fn broken_contexts_fail_without_network(cx: &mut TestAppContext) {
        let (_dir, manager) = setup(cx);
        let broken = manager.read_with(cx, |m, _| m.all_contexts()[3].id.clone());
        manager.update(cx, |m, cx| m.activate(&broken, cx));
        cx.run_until_parked();
        manager.read_with(cx, |m, _| {
            assert!(matches!(
                m.state(&broken),
                ConnectionState::Unreachable { retry_in: None, .. }
            ));
            assert_eq!(m.active(), Some(&broken));
        });
        cx.update(|cx| {
            let active = ActiveContext::global(cx);
            assert_eq!(active.cluster.as_ref().unwrap().name.as_ref(), "broken");
            assert!(!active.cluster.as_ref().unwrap().connected);
        });
    }

    #[gpui::test]
    async fn pasted_kubeconfigs_are_saved_and_loaded(cx: &mut TestAppContext) {
        let (dir, manager) = setup(cx);
        let task = manager.update(cx, |m, cx| {
            m.paste_kubeconfig("team a".into(), OTHER.into(), cx)
        });
        let path = task.await.unwrap();
        assert!(path.starts_with(dir.path().join("pasted")));
        cx.run_until_parked();
        manager.read_with(cx, |m, _| {
            assert!(m.contexts().any(|c| c.name == "prod@team-a"));
        });
        let err = manager.update(cx, |m, cx| {
            m.paste_kubeconfig("x".into(), "nope: [".into(), cx)
        });
        assert!(err.await.is_err());
    }

    #[gpui::test]
    async fn sources_can_be_removed_and_pasted_files_deleted(cx: &mut TestAppContext) {
        let (dir, manager) = setup(cx);
        let other = dir.path().join("kube/other.yaml");
        manager.update(cx, |m, cx| m.remove_source(&other, cx));
        cx.run_until_parked();
        manager.read_with(cx, |m, _| {
            assert_eq!(m.sources().len(), 2);
            assert!(!m.contexts().any(|c| c.name == "prod@other"));
        });
        // Removing a source leaves the file alone.
        assert!(other.exists());

        // The default kubeconfig is a setting, not a file operation.
        manager.update(cx, |m, cx| m.set_load_default_kubeconfig(true, cx));
        cx.run_until_parked();
        cx.update(|cx| assert!(Settings::get::<KubeSettings>(cx).load_default_kubeconfig));
        manager.update(cx, |m, cx| m.set_load_default_kubeconfig(false, cx));

        let pasted = manager
            .update(cx, |m, cx| {
                m.paste_kubeconfig("team".into(), OTHER.into(), cx)
            })
            .await
            .unwrap();
        cx.run_until_parked();
        manager.read_with(cx, |m, _| {
            assert!(m.contexts().any(|c| c.name == "prod@team"));
        });
        manager
            .update(cx, |m, cx| m.delete_pasted(&pasted, cx))
            .await
            .unwrap();
        cx.run_until_parked();
        assert!(!pasted.exists());
        manager.read_with(cx, |m, _| {
            assert!(!m.contexts().any(|c| c.name == "prod@team"));
        });
        // Only files in the pasted folder can be deleted.
        let refused = manager
            .update(cx, |m, cx| m.delete_pasted(&other, cx))
            .await;
        assert!(refused.is_err());
        assert!(other.exists());
    }

    /// A kubeconfig with one `oc`-style context (`oc login`), in its own folder.
    fn oc_setup(
        cx: &mut TestAppContext,
        extra: &str,
    ) -> (
        tempfile::TempDir,
        std::path::PathBuf,
        Entity<ConnectionManager>,
    ) {
        let dir = tempfile::tempdir().unwrap();
        let file = dir.path().join("config");
        std::fs::write(&file, oc_config(&["shop"], "shop")).unwrap();
        let settings = format!(
            r#"{{"kubernetes": {{"load_default_kubeconfig": false, "load_kubeconfig_env": false,
                "kubeconfigs": [{}]{extra}}}}}"#,
            serde_json::to_string(&file).unwrap()
        );
        std::fs::write(dir.path().join("settings.json"), settings).unwrap();
        let manager = cx.update(|cx| {
            kubyl_core::init(cx);
            kubyl_settings::init_with_dir(cx, dir.path());
            kubyl_ui::init(cx);
            Settings::register::<KubeSettings>(cx);
            ConnectionManager::install(dir.path().join("pasted"), false, cx)
        });
        cx.run_until_parked();
        (dir, file, manager)
    }

    /// `oc login` plus one `oc project` per namespace; `current` is the current context's.
    fn oc_config(namespaces: &[&str], current: &str) -> String {
        let cluster = "api-ocp-eu1-example-com:6443";
        let user = "jane.doe@example.com";
        let mut yaml = format!(
            "apiVersion: v1\nkind: Config\ncurrent-context: \"{current}/{cluster}/{user}\"\nclusters:\n- name: \"{cluster}\"\n  cluster: {{server: \"https://api.ocp.eu1.example.com:6443\"}}\nusers:\n- name: \"{user}/{cluster}\"\n  user: {{token: sha256~not-a-real-token}}\ncontexts:\n"
        );
        for ns in namespaces {
            yaml.push_str(&format!(
                "- name: \"{ns}/{cluster}/{user}\"\n  context: {{cluster: \"{cluster}\", user: \"{user}/{cluster}\", namespace: {ns}}}\n"
            ));
        }
        yaml
    }

    impl ConnectionManager {
        /// Pretends `id` is connected (tests never connect).
        fn fake_connected(&mut self, id: &ClusterId) {
            self.clusters.insert(
                id.clone(),
                Cluster {
                    state: ConnectionState::Connected {
                        latency: Duration::from_millis(5),
                        version: "v1.33.1".into(),
                    },
                    generation: 42,
                    ..Default::default()
                },
            );
        }
    }

    #[gpui::test]
    fn a_first_sibling_rekeys_the_connection_without_reconnecting(cx: &mut TestAppContext) {
        let (_dir, file, manager) = oc_setup(cx, "");
        let single = manager.read_with(cx, |m, _| {
            let entries: Vec<_> = m.contexts().collect();
            assert_eq!(entries.len(), 1);
            assert!(!entries[0].is_group());
            // oc-style contexts get the short label even without siblings.
            assert_eq!(
                entries[0].name,
                "ocp.eu1.example.com · jane.doe@example.com"
            );
            entries[0].id.clone()
        });
        manager.update(cx, |m, cx| {
            m.fake_connected(&single);
            m.active = Some(single.clone());
            m.sync_active(cx);
        });
        let events = std::rc::Rc::new(std::cell::RefCell::new(Vec::new()));
        let seen = events.clone();
        let _subscription = cx.update(|cx| {
            cx.subscribe(&manager, move |_, event: &ConnectionEvent, _| {
                seen.borrow_mut().push(event.clone())
            })
        });

        // `oc project payments`: a sibling context, and it becomes the current one.
        std::fs::write(&file, oc_config(&["shop", "payments"], "payments")).unwrap();
        manager.update(cx, |m, cx| m.reload(cx));
        cx.run_until_parked();
        let group = manager.read_with(cx, |m, _| {
            let entries: Vec<_> = m.contexts().collect();
            assert_eq!(entries.len(), 1);
            assert!(entries[0].is_group());
            let group = entries[0].id.clone();
            assert!(group.as_str().starts_with(crate::groups::GROUP_PREFIX));
            // Same connection: not reconnected.
            let cluster = m.cluster(&group).unwrap();
            assert_eq!(cluster.generation, 42);
            assert!(cluster.state.is_connected());
            // The old id and the member ids lead to the group; the active cluster followed.
            assert_eq!(m.resolve(&single), group);
            assert!(m.state(&single).is_connected());
            assert_eq!(m.active(), Some(&group));
            group
        });
        assert!(events.borrow().contains(&ConnectionEvent::Rekeyed {
            from: single.clone(),
            to: group.clone()
        }));
        cx.update(|cx| {
            assert_eq!(
                ActiveContext::global(cx).cluster.as_ref().unwrap().id,
                group
            );
        });

        // Another `oc project`: nothing changes about the entry or the connection.
        std::fs::write(&file, oc_config(&["shop", "payments", "ops"], "ops")).unwrap();
        manager.update(cx, |m, cx| m.reload(cx));
        cx.run_until_parked();
        manager.read_with(cx, |m, _| {
            let entry = m.contexts().next().unwrap();
            assert_eq!(entry.id, group);
            assert_eq!(entry.members.len(), 3);
            assert_eq!(m.cluster(&group).unwrap().generation, 42);
        });

        // The siblings go away again: back to the single context's id, still connected.
        std::fs::write(&file, oc_config(&["shop"], "shop")).unwrap();
        manager.update(cx, |m, cx| m.reload(cx));
        cx.run_until_parked();
        manager.read_with(cx, |m, _| {
            let entry = m.contexts().next().unwrap();
            assert_eq!(entry.id, single);
            assert_eq!(m.cluster(&single).unwrap().generation, 42);
            assert_eq!(m.resolve(&group), single);
        });
    }

    #[gpui::test]
    fn groups_merge_member_settings_and_start_in_the_current_namespace(cx: &mut TestAppContext) {
        let (_dir, file, manager) = oc_setup(cx, "");
        std::fs::write(&file, oc_config(&["shop", "payments", "ops"], "payments")).unwrap();
        manager.update(cx, |m, cx| m.reload(cx));
        cx.run_until_parked();
        let (group, shop) = manager.read_with(cx, |m, _| {
            let entry = m.contexts().next().unwrap();
            let shop = entry
                .members
                .iter()
                .find(|member| member.context.starts_with("shop/"))
                .unwrap()
                .id
                .clone();
            (entry.id.clone(), shop)
        });
        // PROD and a color on one member (old settings from before grouping).
        let shop_key = shop.to_string();
        cx.update(|cx| {
            Settings::update::<KubeSettings>(cx, move |s| {
                let entry = s.contexts.entry(shop_key).or_default();
                entry.production = true;
                entry.color = Some(crate::settings::ColorTag::Green);
            })
        });
        cx.run_until_parked();
        manager.read_with(cx, |m, cx| {
            assert!(m.context_settings(&group).production);
            assert!(m.caps(&group).production);
            assert_eq!(
                m.color(&group, cx),
                crate::settings::ColorTag::Green.color(kubyl_ui::ActiveColors::colors(cx))
            );
            let keys = m.settings_keys(&group);
            assert_eq!(keys[0], group.to_string());
            assert!(keys.contains(&shop.to_string()));
            assert!(keys.iter().any(|k| k.starts_with("shop/api-ocp-eu1")));
        });
        // Turning PROD off on the group clears it on the member too.
        manager.update(cx, |m, cx| {
            m.update_context_settings(&group, cx, |s| s.production = false)
        });
        cx.run_until_parked();
        manager.read_with(cx, |m, _| {
            assert!(!m.context_settings(&group).production);
            assert!(!m.settings.context(shop.as_str()).production);
            // The color stays (written under the group id).
            assert!(m.settings.context(group.as_str()).color.is_some());
        });
        // The file's current context is a member: the group starts in its namespace.
        manager.update(cx, |m, cx| m.activate(&group, cx));
        cx.run_until_parked();
        cx.update(|cx| {
            assert_eq!(
                ActiveContext::global(cx).namespace.as_deref(),
                Some("payments")
            );
        });
        manager.read_with(cx, |m, _| {
            let namespaces = fallback_namespaces(m.context(&group), &Default::default(), None);
            assert_eq!(namespaces.kubeconfig, ["ops", "payments", "shop"]);
        });
    }

    #[gpui::test]
    fn grouping_can_be_turned_off(cx: &mut TestAppContext) {
        let (_dir, file, manager) = oc_setup(cx, r#", "group_contexts": false"#);
        std::fs::write(&file, oc_config(&["shop", "payments"], "shop")).unwrap();
        manager.update(cx, |m, cx| m.reload(cx));
        cx.run_until_parked();
        let group = manager.read_with(cx, |m, _| {
            assert_eq!(m.contexts().count(), 2);
            m.contexts().next().unwrap().group.clone().unwrap()
        });
        // A stored group id finds a member while grouping is off (the current context).
        manager.read_with(cx, |m, _| {
            let resolved = m.resolve(&group);
            assert!(resolved.as_str().starts_with("shop/"), "{resolved}");
        });
        cx.update(|cx| Settings::update::<KubeSettings>(cx, |s| s.group_contexts = true));
        cx.run_until_parked();
        manager.read_with(cx, |m, _| {
            assert_eq!(m.contexts().count(), 1);
            assert_eq!(m.contexts().next().unwrap().id, group);
        });
        // "Show Contexts Separately" for this group only.
        manager.update(cx, |m, cx| m.set_group_separate(&group, true, cx));
        cx.run_until_parked();
        manager.read_with(cx, |m, _| assert_eq!(m.contexts().count(), 2));
    }

    #[gpui::test]
    fn a_group_shown_separately_keeps_its_safety_flags(cx: &mut TestAppContext) {
        let (_dir, file, manager) = oc_setup(cx, "");
        std::fs::write(&file, oc_config(&["shop", "payments"], "shop")).unwrap();
        manager.update(cx, |m, cx| m.reload(cx));
        cx.run_until_parked();
        let group = manager.read_with(cx, |m, _| m.contexts().next().unwrap().id.clone());
        manager.update(cx, |m, cx| {
            m.update_context_settings(&group, cx, |s| {
                s.production = true;
                s.read_only = true;
            })
        });
        manager.update(cx, |m, cx| m.set_group_separate(&group, true, cx));
        cx.run_until_parked();
        let (shop, payments) = manager.read_with(cx, |m, _| {
            let ids: Vec<ClusterId> = m.contexts().map(|c| c.id.clone()).collect();
            assert_eq!(ids.len(), 2);
            for id in &ids {
                let settings = m.context_settings(id);
                assert!(
                    settings.production && settings.read_only,
                    "{id}: {settings:?}"
                );
                assert!(m.caps(id).production && m.caps(id).read_only, "{id}");
            }
            let shop = ids
                .iter()
                .find(|i| i.as_str().starts_with("shop/"))
                .unwrap()
                .clone();
            let payments = ids
                .iter()
                .find(|i| i.as_str().starts_with("payments/"))
                .unwrap()
                .clone();
            (shop, payments)
        });
        // Turning a flag off on one context doesn't turn it off for the other.
        manager.update(cx, |m, cx| {
            m.update_context_settings(&shop, cx, |s| s.read_only = false)
        });
        cx.run_until_parked();
        manager.read_with(cx, |m, _| {
            assert!(!m.context_settings(&shop).read_only);
            assert!(m.context_settings(&shop).production, "untouched");
            assert!(m.context_settings(&payments).read_only);
        });
        // Grouped again, the group is read-only because a member is (production stays too).
        manager.update(cx, |m, cx| m.set_group_separate(&group, false, cx));
        cx.run_until_parked();
        manager.read_with(cx, |m, _| {
            let settings = m.context_settings(&group);
            assert!(settings.production && settings.read_only);
        });
    }

    #[test]
    fn backoff_doubles_up_to_a_minute() {
        assert_eq!(backoff(1), Duration::from_secs(5));
        assert_eq!(backoff(2), Duration::from_secs(10));
        assert_eq!(backoff(10), Duration::from_secs(60));
    }

    #[test]
    fn fallback_namespaces_combine_sources() {
        let settings = ContextSettings {
            default_namespace: Some("payments".into()),
            namespaces: vec!["ops".into(), "payments".into()],
            ..Default::default()
        };
        let ns = fallback_namespaces(None, &settings, Some("default"));
        assert_eq!(ns.names, ["default", "ops", "payments"]);
        assert!(!ns.listed);
    }
}
