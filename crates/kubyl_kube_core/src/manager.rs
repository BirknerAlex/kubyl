//! [`ManagerCore`]: kubeconfig sources, contexts and one connection per cluster, as a plain
//! state machine. `kubyl_kube::ConnectionManager` holds one in a GPUI entity and derefs to it.
//!
//! The core never touches app globals. It asks its [`Host`] to run work, emit
//! [`ConnectionEvent`]s and carry out [`KubeEffect`]s (state.json, settings.json, the title bar).

use std::collections::{BTreeMap, HashMap, HashSet};
use std::future::Future;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::Duration;

use futures::channel::mpsc;
use k8s_openapi::apimachinery::pkg::version::Info;
use kube::config::Kubeconfig;
use kubyl_base::host::{Flow, Host, HostExt as _, Pace, Service, TaskHandle};
use kubyl_base::{ClusterCaps, ClusterId, Notice, TabContext, TabNamespace};
use kubyl_settings_core::StateSection;
use serde::{Deserialize, Serialize};

use crate::access::{AccessCache, AccessQuery};
use crate::auth::CredentialSource;
use crate::client::{self, BuiltClient, ConnectError, Probe};
use crate::cluster_info::{self, ClusterInfo};
use crate::discovery::{self, Discovery};
use crate::groups::{self, GroupOptions};
use crate::kubeconfig::{self, ContextInfo, Loaded, Source, SourceSpec};
use crate::openapi;
use crate::settings::{
    ColorTag, ContextSettings, KubeSettings, context_color_tag, display_path, expand_home,
};
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
    /// Watches, retries and in-flight requests. Dropping them aborts them.
    tasks: Vec<TaskHandle>,
    /// The health loop's next step (a timer or a ping).
    health: Option<TaskHandle>,
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
            health: None,
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

    /// Bumped on every (re)connect.
    pub fn generation(&self) -> u64 {
        self.generation
    }
}

/// What changed. GPUI views subscribe to `kubyl_kube::ConnectionManager`, which re-emits these.
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
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct KubeState {
    pub active: Option<ClusterId>,
    /// The namespace last used in each group (where a group starts after a restart when the
    /// file's current context isn't one of its members).
    #[serde(skip_serializing_if = "BTreeMap::is_empty")]
    pub namespaces: BTreeMap<String, String>,
}

impl StateSection for KubeState {
    const KEY: &'static str = "kube";
}

/// App-level effects the core asks its host for.
pub enum KubeEffect {
    /// Store this in state.json.
    SaveState(KubeState),
    /// Change the `"kubernetes"` section of settings.json (the host feeds the result back
    /// through [`ManagerCore::settings_changed`]).
    UpdateSettings(Box<dyn FnOnce(&mut KubeSettings)>),
    /// The title bar's cluster badge may be out of date ([`ManagerCore::badge_info`]).
    SyncActive,
    /// Show `id` in the title bar with `namespace`. `from_tab`: an activated tab chose it.
    Activated {
        id: ClusterId,
        namespace: Option<String>,
        from_tab: bool,
    },
    /// No cluster is active anymore.
    ClearActive,
    /// Cluster ids may resolve differently now (see [`ManagerCore::resolve`]).
    ClusterIdsChanged,
}

/// How the chrome shows a cluster, without theme colors.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct BadgeInfo {
    pub id: ClusterId,
    pub name: String,
    pub color: ColorTag,
    pub production: bool,
    pub meta: Option<String>,
    pub connected: bool,
}

/// Owns kubeconfig sources, the merged context list and one connection per cluster.
pub struct ManagerCore {
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
    /// Bumped by every [`Self::reload`]; only the latest load's result is applied.
    reload_generation: u64,
    watch_files: bool,
    watcher: Option<notify::RecommendedWatcher>,
    watched: Vec<PathBuf>,
    file_events: Option<mpsc::UnboundedSender<()>>,
    settings: KubeSettings,
    /// The `"kube"` section of state.json (only this core writes it).
    state: KubeState,
    /// Clusters the user explicitly asked for; an OIDC sign-in prompt is shown for them.
    interactive: HashSet<ClusterId>,
    tasks: Vec<TaskHandle>,
}

impl Service for ManagerCore {
    type Event = ConnectionEvent;
    type Effect = KubeEffect;
}

impl ManagerCore {
    /// A manager with nothing loaded yet. Call [`Self::start`] once it has a host.
    pub fn new(
        pasted_dir: PathBuf,
        watch_files: bool,
        settings: KubeSettings,
        state: KubeState,
    ) -> Self {
        Self {
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
            reload_generation: 0,
            watch_files,
            watcher: None,
            watched: Vec::new(),
            file_events: None,
            settings,
            state,
            interactive: HashSet::new(),
            tasks: Vec::new(),
        }
    }

    /// Starts watching kubeconfig files (if enabled) and loads them.
    pub fn start(&mut self, host: &mut dyn Host<Self>) {
        if self.watch_files {
            self.start_file_events(host);
        }
        self.reload(host);
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

    /// The `"kubernetes"` settings this manager runs with.
    pub fn settings(&self) -> &KubeSettings {
        &self.settings
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
    pub fn display_name(&self, id: &ClusterId) -> String {
        self.context_settings(id)
            .display_name
            .or_else(|| self.context(id).map(|c| c.name.clone()))
            .unwrap_or_else(|| id.to_string())
    }

    /// The color tag of an entry: its own, or a stable default.
    pub fn color_tag(&self, id: &ClusterId) -> ColorTag {
        let id = self.resolve(id);
        context_color_tag(id.as_str(), &self.context_settings(&id))
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
    pub fn reload(&mut self, host: &mut dyn Host<Self>) {
        let specs = kubeconfig::source_specs(
            &self.settings,
            std::env::var_os("KUBECONFIG"),
            dirs::home_dir(),
            &self.pasted_dir,
        );
        self.loading = true;
        self.reload_generation += 1;
        let generation = self.reload_generation;
        self.tasks.push(host.background(
            async move { (kubeconfig::load(&specs), specs) },
            move |this, (loaded, specs), host| this.finish_reload(generation, loaded, &specs, host),
        ));
        // Finished loads stay in the list until the next reload; keep it short.
        if self.tasks.len() > 8 {
            self.tasks.drain(..self.tasks.len() - 4);
        }
    }

    /// Applies a finished load unless a newer reload started since (its result is coming and
    /// would be overwritten by this stale one).
    fn finish_reload(
        &mut self,
        generation: u64,
        loaded: Loaded,
        specs: &[SourceSpec],
        host: &mut dyn Host<Self>,
    ) {
        if generation == self.reload_generation {
            self.apply_loaded(loaded, specs, host);
        }
    }

    fn apply_loaded(&mut self, loaded: Loaded, specs: &[SourceSpec], host: &mut dyn Host<Self>) {
        self.loading = false;
        tracing::info!(
            sources = loaded.sources.len(),
            contexts = loaded.contexts.len(),
            "kubeconfigs loaded"
        );
        self.loaded = loaded;
        self.apply_entries(host);
        if self.watch_files {
            self.update_watcher(specs, host);
        }
        if !self.restored {
            self.restored = true;
            if let Some(id) = self.state.active.clone() {
                let id = self.resolve(&id);
                if self.context(&id).is_some() {
                    self.activate(&id, host);
                }
            }
        }
        host.effect(KubeEffect::SyncActive);
        host.emit(ConnectionEvent::ContextsChanged);
        host.notify();
    }

    /// Rebuilds the entries from the loaded contexts and the grouping settings. Connections of
    /// entries whose id changed (a context got its first sibling, or lost its last one) move to
    /// the new id without reconnecting; removed ones are dropped; changed ones reconnect. A new
    /// namespace or current context changes nothing about a connection.
    fn apply_entries(&mut self, host: &mut dyn Host<Self>) {
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
                self.state.active = Some(to.clone());
                host.effect(KubeEffect::SaveState(self.state.clone()));
            }
            host.emit(ConnectionEvent::Rekeyed {
                from: from.clone(),
                to: to.clone(),
            });
            host.emit(ConnectionEvent::StateChanged(to.clone()));
        }
        for id in removed {
            host.emit(ConnectionEvent::StateChanged(id));
        }
        for id in changed {
            tracing::info!(context = %id, "kubeconfig changed; reconnecting");
            self.connect(&id, host);
        }
        // Members and their namespaces may have changed.
        let ids: Vec<ClusterId> = self.clusters.keys().cloned().collect();
        for id in ids {
            self.refresh_fallback_namespaces(&id, host);
        }
        if self
            .active
            .as_ref()
            .is_some_and(|id| self.context(id).is_none())
        {
            self.active = None;
            host.effect(KubeEffect::ClearActive);
            host.emit(ConnectionEvent::ActiveChanged(None));
        }
        // Tabs with ids that resolve differently now are rebuilt by the workspace.
        host.effect(KubeEffect::ClusterIdsChanged);
    }

    /// The namespaces of an entry whose listing is forbidden, after its settings or members
    /// changed.
    fn refresh_fallback_namespaces(&mut self, id: &ClusterId, host: &mut dyn Host<Self>) {
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
                host.emit(ConnectionEvent::NamespacesChanged(id.clone()));
            }
            return;
        }
        let namespaces = fallback_namespaces(info.as_ref(), &settings, cluster.default_namespace());
        if cluster.namespaces != namespaces {
            cluster.namespaces = namespaces;
            host.emit(ConnectionEvent::NamespacesChanged(id.clone()));
        }
    }

    /// The `"kubernetes"` settings changed (edited, or a [`KubeEffect::UpdateSettings`]).
    pub fn settings_changed(&mut self, new: KubeSettings, host: &mut dyn Host<Self>) {
        let old = std::mem::replace(&mut self.settings, new.clone());
        if old.kubeconfigs != new.kubeconfigs
            || old.load_default_kubeconfig != new.load_default_kubeconfig
            || old.load_kubeconfig_env != new.load_kubeconfig_env
        {
            self.reload(host);
        }
        let regroup =
            old.group_contexts != new.group_contexts || old.separate_groups != new.separate_groups;
        if regroup {
            self.apply_entries(host);
        }
        if old.contexts != new.contexts || regroup {
            let ids: Vec<ClusterId> = self.clusters.keys().cloned().collect();
            for id in ids {
                self.refresh_caps(&id);
                self.refresh_fallback_namespaces(&id, host);
            }
            host.effect(KubeEffect::SyncActive);
            host.emit(ConnectionEvent::ContextsChanged);
        }
        host.notify();
    }

    fn update_settings(host: &mut dyn Host<Self>, f: impl FnOnce(&mut KubeSettings) + 'static) {
        host.effect(KubeEffect::UpdateSettings(Box::new(f)));
    }

    /// Adds kubeconfig files or folders as sources (stored in settings.json).
    pub fn add_sources(&mut self, paths: Vec<PathBuf>, host: &mut dyn Host<Self>) {
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
            host.toast(Notice::info("Those kubeconfigs are already loaded."));
            return;
        }
        Self::update_settings(host, move |settings| {
            for path in new {
                if !settings.kubeconfigs.contains(&path) {
                    settings.kubeconfigs.push(path);
                }
            }
        });
    }

    /// Removes a user-added source. Files are never deleted, except pasted ones.
    pub fn remove_source(&mut self, path: &Path, host: &mut dyn Host<Self>) {
        let display = display_path(path);
        let path_str = path.display().to_string();
        let path = path.to_path_buf();
        Self::update_settings(host, move |settings| {
            settings
                .kubeconfigs
                .retain(|p| p != &path_str && p != &display && expand_home(p) != path);
        });
    }

    /// Turns loading `~/.kube/config` on or off (settings.json). The file itself is untouched.
    pub fn set_load_default_kubeconfig(&mut self, load: bool, host: &mut dyn Host<Self>) {
        Self::update_settings(host, move |settings| {
            settings.load_default_kubeconfig = load
        });
    }

    /// Turns loading the files in `$KUBECONFIG` on or off (settings.json).
    pub fn set_load_kubeconfig_env(&mut self, load: bool, host: &mut dyn Host<Self>) {
        Self::update_settings(host, move |settings| settings.load_kubeconfig_env = load);
    }

    /// Checks that `path` is a pasted kubeconfig, the copy Kubyl keeps in [`Self::pasted_dir`]:
    /// Kubyl never deletes the user's own kubeconfig files.
    pub fn check_pasted(&self, path: &Path) -> Result<(), String> {
        if path.parent() == Some(self.pasted_dir.as_path()) {
            Ok(())
        } else {
            Err(format!("{} isn't a pasted kubeconfig", display_path(path)))
        }
    }

    /// Changes an entry's overrides in settings.json. A group's are written under the group
    /// id, starting from the merged settings; a safety flag turned off on a group is cleared on
    /// its members too (otherwise any member would keep it on).
    pub fn update_context_settings(
        &mut self,
        id: &ClusterId,
        host: &mut dyn Host<Self>,
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
        Self::update_settings(host, move |stored| {
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
    pub fn set_group_contexts(&mut self, group: bool, host: &mut dyn Host<Self>) {
        Self::update_settings(host, move |settings| settings.group_contexts = group);
    }

    /// Shows a group's contexts as separate entries, or (`separate: false`) as one again.
    pub fn set_group_separate(
        &mut self,
        group: &ClusterId,
        separate: bool,
        host: &mut dyn Host<Self>,
    ) {
        let group = group.to_string();
        Self::update_settings(host, move |settings| {
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
        host: &mut dyn Host<Self>,
    ) {
        let (from, to) = (from.to_string(), to.to_string());
        if from == to {
            return;
        }
        Self::update_settings(host, move |settings| {
            if let Some(entry) = settings.contexts.remove(&from) {
                settings.contexts.insert(to, entry);
            }
        });
    }

    // ----- File watching -----

    fn start_file_events(&mut self, host: &mut dyn Host<Self>) {
        let (tx, rx) = mpsc::unbounded::<()>();
        self.file_events = Some(tx);
        self.tasks.push(host.batches(
            rx,
            Pace::debounce(RELOAD_DEBOUNCE),
            |this, _events, host| {
                this.reload(host);
                Flow::Continue
            },
        ));
    }

    /// Watches the parent folders of source files (editors replace files by renaming, which
    /// ends a watch on the file itself) and folder sources. The file system work runs in the
    /// background.
    fn update_watcher(&mut self, specs: &[SourceSpec], host: &mut dyn Host<Self>) {
        let Some(tx) = self.file_events.clone() else {
            return;
        };
        let specs = specs.to_vec();
        let watched = self.watched.clone();
        let watching = self.watcher.is_some();
        let generation = self.reload_generation;
        self.tasks.push(host.background(
            async move { build_watcher(&specs, &watched, watching, tx) },
            move |this, built, _| {
                // A newer reload plans its own watcher.
                if let Some((watcher, dirs)) = built
                    && this.reload_generation == generation
                {
                    this.watcher = Some(watcher);
                    this.watched = dirs;
                }
            },
        ));
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

    /// The current connection of `id`, if `generation` is still current.
    fn current(&mut self, id: &ClusterId, generation: u64) -> Option<&mut Cluster> {
        let key = self.key(id);
        self.clusters
            .get_mut(&key)
            .filter(|c| c.generation == generation)
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

    /// Capabilities, including the user's production and read-only flags. Those flags come from
    /// the settings, so they also hold while the cluster is disconnected (a reset connection
    /// starts with default caps).
    pub fn caps(&self, id: &ClusterId) -> ClusterCaps {
        let settings = self.context_settings(id);
        match self.clusters.get(&self.key(id)) {
            Some(cluster) => ClusterCaps {
                read_only: settings.read_only,
                production: settings.production,
                ..cluster.caps.clone()
            },
            None => cluster_info::caps(None, None, &settings),
        }
    }

    /// Number of clusters currently connected.
    pub fn connected_count(&self) -> usize {
        self.clusters
            .values()
            .filter(|c| c.state.is_connected())
            .count()
    }

    /// Open API server connections of each cluster that has a client, by display name.
    pub fn open_connections(&self) -> Vec<(String, crate::transport::OpenConnections)> {
        let mut list: Vec<_> = self
            .clusters
            .iter()
            .filter_map(|(id, cluster)| {
                let open = cluster.client.as_ref()?.connections.snapshot();
                (open.total() > 0).then(|| (self.display_name(id), open))
            })
            .collect();
        list.sort_by(|a, b| a.0.cmp(&b.0));
        list
    }

    pub fn active(&self) -> Option<&ClusterId> {
        self.active.as_ref()
    }

    /// Connects unless already connected or connecting. Use when a cluster root is expanded or
    /// a favorite is opened.
    pub fn ensure_connected(&mut self, id: &ClusterId, host: &mut dyn Host<Self>) {
        let id = &self.resolve(id);
        match self.state(id) {
            ConnectionState::Disconnected
            | ConnectionState::Unreachable { .. }
            | ConnectionState::Forbidden(_) => self.connect(id, host),
            ConnectionState::AuthRequired { sign_in: false, .. } => self.connect(id, host),
            _ => {}
        }
    }

    /// (Re)connects: builds a client, checks it, then starts discovery, watches and the health
    /// ping.
    pub fn connect(&mut self, id: &ClusterId, host: &mut dyn Host<Self>) {
        let id = &self.resolve(id);
        let Some(info) = self.context(id).cloned() else {
            return;
        };
        let config = self.loaded.configs.get(&info.file).cloned();
        let cluster = self.clusters.entry(id.clone()).or_default();
        cluster.generation += 1;
        let generation = cluster.generation;
        // Aborts the old client's watches and pings.
        cluster.tasks = Vec::new();
        cluster.health = None;
        cluster.client = None;
        cluster.discovery = None;
        cluster.crd_count = None;
        cluster.crds.clear();
        cluster.unserved_retries = 0;
        cluster.access.clear();
        let (Some(config), None) = (config, info.error.clone()) else {
            let message = info
                .error
                .clone()
                .unwrap_or_else(|| "kubeconfig not loaded".into());
            cluster.state = ConnectionState::Unreachable {
                message,
                retry_in: None,
            };
            host.emit(ConnectionEvent::StateChanged(id.clone()));
            host.effect(KubeEffect::SyncActive);
            host.notify();
            return;
        };
        cluster.state = ConnectionState::Connecting;

        let task_info = info.clone();
        let connect = async move {
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
        };
        let id_task = id.clone();
        let task = host.spawn(connect, move |this, result, host| {
            this.finish_connect(&id_task, generation, result, host)
        });
        if let Some(cluster) = self.clusters.get_mut(id) {
            cluster.tasks.push(task);
        }
        host.emit(ConnectionEvent::StateChanged(id.clone()));
        host.effect(KubeEffect::SyncActive);
        host.notify();
    }

    /// Disconnects and forgets the client (watches stop).
    pub fn disconnect(&mut self, id: &ClusterId, host: &mut dyn Host<Self>) {
        let id = &self.key(id);
        let key = self.key(id);
        if let Some(cluster) = self.clusters.get_mut(&key) {
            cluster.generation += 1;
            *cluster = Cluster {
                generation: cluster.generation,
                ..Default::default()
            };
            host.emit(ConnectionEvent::StateChanged(id.clone()));
            host.effect(KubeEffect::SyncActive);
            host.notify();
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
        host: &mut dyn Host<Self>,
    ) {
        let id = &self.key(id);
        let Some(info) = self.context(id).cloned() else {
            return;
        };
        let settings = self.context_settings(id);
        let Some(cluster) = self.current(id, generation) else {
            return;
        };
        let (built, probe, expires_at) = match result {
            Ok((built, Ok(probe), expires_at)) => (built, probe, expires_at),
            Ok((built, Err(err), _)) => {
                // Keep the client: an OIDC sign-in reuses its credential source.
                cluster.client = Some(built);
                self.fail(id, generation, err, host);
                return;
            }
            Err(err) => {
                self.fail(id, generation, err, host);
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
        let key = self.key(id);
        let Some(cluster) = self.clusters.get_mut(&key) else {
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

        self.start_watches(id, generation, kube_client.clone(), host);
        self.run_discovery(id, generation, kube_client, host);
        self.start_health(id, generation, host);
        host.emit(ConnectionEvent::StateChanged(id.clone()));
        host.emit(ConnectionEvent::NamespacesChanged(id.clone()));
        host.effect(KubeEffect::SyncActive);
        host.notify();
    }

    fn fail(
        &mut self,
        id: &ClusterId,
        generation: u64,
        err: ConnectError,
        host: &mut dyn Host<Self>,
    ) {
        let id = &self.key(id);
        let sign_in = self.context(id).is_some_and(|c| c.auth.supports_sign_in());
        let name = self.display_name(id);
        let Some(cluster) = self.current(id, generation) else {
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
            host.emit(ConnectionEvent::SignInRequested(id.clone()));
        }
        let key = self.key(id);
        let Some(cluster) = self.clusters.get_mut(&key) else {
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
            host.toast(Notice::error(message).title(if was_connected {
                "Connection lost"
            } else {
                "Couldn't connect"
            }));
        }
        if let ConnectionState::Unreachable {
            retry_in: Some(delay),
            ..
        } = cluster.state
        {
            let id = id.clone();
            cluster.tasks.push(host.after(delay, move |this, host| {
                let current = this.clusters.get(&this.key(&id)).map(|c| c.generation);
                if current == Some(generation) {
                    // Replaces (and so cancels) this task; nothing runs after it.
                    this.connect(&id, host);
                }
            }));
        }
        host.emit(ConnectionEvent::StateChanged(id.clone()));
        host.effect(KubeEffect::SyncActive);
        host.notify();
    }

    /// Pings the cluster every `health_check_interval` (backing off while it fails) and
    /// rebuilds the client shortly before an exec client certificate expires.
    fn start_health(&mut self, id: &ClusterId, generation: u64, host: &mut dyn Host<Self>) {
        let interval = Duration::from_secs(self.settings.health_check_interval.max(5));
        self.schedule_health(id, generation, interval, interval, host);
    }

    fn schedule_health(
        &mut self,
        id: &ClusterId,
        generation: u64,
        interval: Duration,
        delay: Duration,
        host: &mut dyn Host<Self>,
    ) {
        let task_id = id.clone();
        let task = host.after(delay, move |this, host| {
            this.health_tick(&task_id, generation, interval, host)
        });
        if let Some(cluster) = self.current(id, generation) {
            // Replaces the step that called this; nothing runs after it.
            cluster.health = Some(task);
        }
    }

    fn health_tick(
        &mut self,
        id: &ClusterId,
        generation: u64,
        interval: Duration,
        host: &mut dyn Host<Self>,
    ) {
        let Some(cluster) = self.current(id, generation) else {
            return;
        };
        let Some(built) = cluster.client.as_ref() else {
            return;
        };
        let (client, rebuild_at) = (built.client.clone(), built.rebuild_at);
        if rebuild_at.is_some_and(|at| jiff::Timestamp::now() + Duration::from_secs(60) >= at) {
            // An exec client certificate is about to expire.
            self.connect(id, host);
            return;
        }
        let task_id = id.clone();
        let task = host.spawn(
            async move { client::ping(&client).await },
            move |this, ping, host| {
                let id = &task_id;
                let ok = this.finish_ping(id, generation, ping, host);
                if this.current(id, generation).is_none() {
                    return;
                }
                let delay = if ok {
                    interval
                } else {
                    let failures = this.clusters.get(&this.key(id)).map_or(1, |c| c.failures);
                    backoff(failures)
                };
                this.schedule_health(id, generation, interval, delay, host);
            },
        );
        if let Some(cluster) = self.current(id, generation) {
            cluster.health = Some(task);
        }
    }

    /// Keeps `task` alive as long as this connection attempt is current.
    fn push_task(&mut self, id: &ClusterId, generation: u64, task: TaskHandle) {
        if let Some(cluster) = self.current(id, generation) {
            cluster.tasks.push(task);
        }
    }

    /// Returns whether the cluster is healthy.
    fn finish_ping(
        &mut self,
        id: &ClusterId,
        generation: u64,
        ping: Result<Duration, ConnectError>,
        host: &mut dyn Host<Self>,
    ) -> bool {
        let id = &self.key(id);
        let Some(cluster) = self.current(id, generation) else {
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
                    host.toast(Notice::success(format!("{name} is reachable again")));
                }
                host.emit(ConnectionEvent::StateChanged(id.clone()));
                host.effect(KubeEffect::SyncActive);
                host.notify();
                true
            }
            Err(err) => {
                let sign_in = self.context(id).is_some_and(|c| c.auth.supports_sign_in());
                let name = self.display_name(id);
                let key = self.key(id);
                let cluster = self.clusters.get_mut(&key).expect("checked above");
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
                    host.toast(Notice::warning(format!("{name}: {err}")).title("Connection lost"));
                }
                host.emit(ConnectionEvent::StateChanged(id.clone()));
                host.effect(KubeEffect::SyncActive);
                host.notify();
                false
            }
        }
    }

    fn start_watches(
        &mut self,
        id: &ClusterId,
        generation: u64,
        client: kube::Client,
        host: &mut dyn Host<Self>,
    ) {
        // Namespaces.
        let (tx, rx) = mpsc::unbounded::<NamespaceUpdate>();
        let watch = host.spawn(watches::watch_namespaces(client.clone(), tx), |_, (), _| {});
        let id_ns = id.clone();
        let apply = host.batches(
            rx,
            Pace::throttle(BATCH_INTERVAL),
            move |this, updates: Vec<NamespaceUpdate>, host| {
                if let Some(update) = updates.into_iter().last() {
                    this.apply_namespaces(&id_ns, generation, update, host);
                }
                Flow::Continue
            },
        );

        // CRDs: every change after the first list re-runs discovery.
        let (crd_tx, crd_rx) = mpsc::unbounded::<Vec<String>>();
        let crd_watch = host.spawn(watches::watch_crds(client.clone(), crd_tx), |_, (), _| {});
        let id_crd = id.clone();
        let mut first = true;
        let crds = host.batches(
            crd_rx,
            Pace::debounce_after_first(CRD_DEBOUNCE),
            move |this, lists: Vec<Vec<String>>, host| {
                let Some(crds) = lists.into_iter().last() else {
                    return Flow::Continue;
                };
                let rediscover = !first;
                first = false;
                let id_crd = this.key(&id_crd);
                let Some(cluster) = this.current(&id_crd, generation) else {
                    return Flow::Stop;
                };
                cluster.crd_count = Some(crds.len());
                cluster.crds = crds;
                if rediscover {
                    tracing::info!(context = %id_crd, "CRDs changed; re-running discovery");
                    cluster.unserved_retries = 0;
                    this.run_discovery(&id_crd, generation, client.clone(), host);
                } else {
                    this.rediscover_unserved(&id_crd, generation, client.clone(), host);
                }
                host.emit(ConnectionEvent::DiscoveryChanged(id_crd.clone()));
                host.notify();
                Flow::Continue
            },
        );
        for task in [watch, apply, crd_watch, crds] {
            self.push_task(id, generation, task);
        }
    }

    fn apply_namespaces(
        &mut self,
        id: &ClusterId,
        generation: u64,
        update: NamespaceUpdate,
        host: &mut dyn Host<Self>,
    ) {
        let id = &self.key(id);
        let settings = self.context_settings(id);
        let info = self.context(id).cloned();
        let Some(cluster) = self.current(id, generation) else {
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
        host.emit(ConnectionEvent::NamespacesChanged(id.clone()));
        host.notify();
    }

    /// A new CRD is only served once it's Established, a status update the CRD watch doesn't
    /// report. While a CRD the watch listed isn't in discovery, discovery re-runs (a few times).
    fn rediscover_unserved(
        &mut self,
        id: &ClusterId,
        generation: u64,
        client: kube::Client,
        host: &mut dyn Host<Self>,
    ) {
        let id = &self.key(id);
        let Some(cluster) = self.current(id, generation) else {
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
        let task_id = id.clone();
        let task = host.after(delay, move |this, host| {
            if this
                .clusters
                .get(&this.key(&task_id))
                .is_some_and(|c| c.generation == generation)
            {
                tracing::debug!(context = %task_id, unserved, "CRDs not served yet; re-running discovery");
                this.run_discovery(&task_id, generation, client, host);
            }
        });
        self.push_task(id, generation, task);
    }

    fn run_discovery(
        &mut self,
        id: &ClusterId,
        generation: u64,
        client: kube::Client,
        host: &mut dyn Host<Self>,
    ) {
        let retry_client = client.clone();
        let task_id = id.clone();
        let task = host.spawn(
            async move { discovery::discover(&client).await },
            move |this, result, host| {
                let id = this.key(&task_id);
                let settings = this.context_settings(&id);
                let info = this.context(&id).cloned();
                let Some(cluster) = this.current(&id, generation) else {
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
                        host.emit(ConnectionEvent::DiscoveryChanged(id.clone()));
                        host.effect(KubeEffect::SyncActive);
                        this.rediscover_unserved(&id, generation, retry_client, host);
                        host.notify();
                    }
                    Err(err) => tracing::warn!(context = %id, "discovery failed: {err}"),
                }
            },
        );
        self.push_task(id, generation, task);
    }

    fn refresh_caps(&mut self, id: &ClusterId) {
        let id = &self.key(id);
        let settings = self.context_settings(id);
        let key = self.key(id);
        if let Some(cluster) = self.clusters.get_mut(&key) {
            cluster.caps = cluster_info::caps(
                cluster.discovery.as_deref(),
                cluster.info.as_ref(),
                &settings,
            );
        }
    }

    // ----- RBAC and OpenAPI -----

    /// A cached answer, without a request.
    pub fn cached_can_i(&self, id: &ClusterId, query: &AccessQuery) -> Option<bool> {
        self.clusters.get(&self.key(id))?.access.get(query)
    }

    /// What an access check for `id` needs: the client and the connection's generation (pass
    /// it back to [`Self::record_access`]). `None` when not connected.
    pub fn access_check(&self, id: &ClusterId) -> Option<(kube::Client, Option<u64>)> {
        let client = self.client(id)?;
        Some((
            client,
            self.clusters.get(&self.key(id)).map(|c| c.generation),
        ))
    }

    /// Caches an answer of [`access::check`] for the connection it was asked on.
    pub fn record_access(
        &mut self,
        id: &ClusterId,
        generation: Option<u64>,
        query: AccessQuery,
        allowed: bool,
    ) {
        let key = self.key(id);
        if let Some(cluster) = self
            .clusters
            .get_mut(&key)
            .filter(|c| Some(c.generation) == generation)
        {
            cluster.access.insert(query, allowed);
        }
    }

    /// Fetches the OpenAPI v3 spec of a group-version (`OpenApiIndex::key(group, version)`),
    /// from the on-disk cache when unchanged. Run it on the Tokio runtime.
    pub fn openapi_request(
        &self,
        id: &ClusterId,
        path: String,
    ) -> Result<impl Future<Output = Result<serde_json::Value, String>> + Send + 'static, String>
    {
        let client = self.client(id).ok_or("not connected")?;
        let server = self
            .context(id)
            .and_then(|c| c.server.clone())
            .unwrap_or_default();
        let dir = openapi::cache_dir(&format!("{id}|{server}"));
        Ok(async move {
            openapi::spec(&client, &dir, &path)
                .await
                .map_err(|e| e.to_string())
        })
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
    pub fn connect_interactive(&mut self, id: &ClusterId, host: &mut dyn Host<Self>) {
        self.interactive.insert(id.clone());
        self.connect(id, host);
    }

    // ----- Active cluster -----

    /// Makes `id` the active cluster (title bar, status bar) and connects it.
    pub fn activate(&mut self, id: &ClusterId, host: &mut dyn Host<Self>) {
        let id = &self.resolve(id);
        if self.context(id).is_none() {
            return;
        }
        let namespace = self.initial_namespace(id);
        self.activate_in(id, namespace, false, host);
    }

    /// Shows the context of an activated tab in the title bar: its cluster, and its namespace
    /// unless the tab leaves that to the title bar. Views in other tabs keep their namespace.
    /// `shown` is what the title bar shows now (cluster and namespace).
    pub fn follow_tab(
        &mut self,
        context: &TabContext,
        shown: (Option<&ClusterId>, Option<&str>),
        host: &mut dyn Host<Self>,
    ) {
        let id = &self.resolve(&context.cluster);
        if self.context(id).is_none() {
            return;
        }
        let (shown_cluster, shown_namespace) = shown;
        let same_cluster = shown_cluster == Some(id);
        let namespace = match &context.namespace {
            TabNamespace::One(namespace) => Some(namespace.clone()),
            TabNamespace::All => None,
            TabNamespace::Keep if same_cluster => return,
            TabNamespace::Keep => self.initial_namespace(id),
        };
        if same_cluster
            && self.active.as_ref() == Some(id)
            && shown_namespace == namespace.as_deref()
        {
            return;
        }
        self.activate_in(id, namespace, true, host);
    }

    fn activate_in(
        &mut self,
        id: &ClusterId,
        namespace: Option<String>,
        from_tab: bool,
        host: &mut dyn Host<Self>,
    ) {
        let changed = self.active.as_ref() != Some(id);
        self.active = Some(id.clone());
        self.state.active = Some(id.clone());
        host.effect(KubeEffect::SaveState(self.state.clone()));
        host.effect(KubeEffect::Activated {
            id: id.clone(),
            namespace,
            from_tab,
        });
        if changed {
            host.emit(ConnectionEvent::ActiveChanged(Some(id.clone())));
        }
        self.interactive.insert(id.clone());
        if let ConnectionState::AuthRequired { sign_in: true, .. } = self.state(id) {
            self.interactive.remove(id);
            host.emit(ConnectionEvent::SignInRequested(id.clone()));
        }
        self.ensure_connected(id, host);
        host.notify();
    }

    /// Where an entry starts: its default namespace, else its kubeconfig namespace. A group
    /// starts in the file's current context's namespace when that context is a member (after
    /// `oc project foo` Kubyl opens `foo` next time), else in the last namespace used in Kubyl,
    /// else in its first member's.
    fn initial_namespace(&self, id: &ClusterId) -> Option<String> {
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
        if let Some(namespace) = self.state.namespaces.get(entry.id.as_str()) {
            return Some(namespace.clone());
        }
        entry
            .members
            .iter()
            .find(|m| m.context == entry.context)
            .and_then(|m| m.namespace.clone())
    }

    /// Keeps the last namespace used in each group (state.json). Call when the title bar's
    /// cluster or namespace changes.
    pub fn remember_namespace(
        &mut self,
        id: &ClusterId,
        namespace: &str,
        host: &mut dyn Host<Self>,
    ) {
        if !self.context(id).is_some_and(ContextInfo::is_group) {
            return;
        }
        if self.state.namespaces.get(id.as_str()).map(String::as_str) == Some(namespace) {
            return;
        }
        self.state
            .namespaces
            .insert(id.to_string(), namespace.to_string());
        host.effect(KubeEffect::SaveState(self.state.clone()));
    }

    /// How the chrome shows a cluster (the GPUI side adds theme colors).
    pub fn badge_info(&self, id: &ClusterId) -> BadgeInfo {
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
        BadgeInfo {
            id: id.clone(),
            name: self.display_name(id),
            color: self.color_tag(id),
            production: settings.production,
            meta,
            connected: state.is_connected(),
        }
    }

    /// Pretends `id` is connected (tests never connect).
    #[cfg(any(test, feature = "test-support"))]
    pub fn fake_connected(&mut self, id: &ClusterId) {
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

    /// Makes `id` active without connecting or telling anyone (tests).
    #[cfg(any(test, feature = "test-support"))]
    pub fn set_active_for_tests(&mut self, id: Option<ClusterId>) {
        self.active = id;
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
pub fn fallback_namespaces(
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

/// See [`ManagerCore::resolve`].
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

/// The folders to watch for `specs`, and the paths whose events matter. Touches the file
/// system (creates folder sources, canonicalizes), so keep it off the UI thread.
fn watch_plan(specs: &[SourceSpec]) -> (Vec<PathBuf>, HashSet<PathBuf>) {
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
    (dirs, relevant)
}

/// A file watcher for `specs` that pings `tx` on changes, or `None` when `watched` is already
/// watched by a running watcher (`watching`) or the watcher can't start.
fn build_watcher(
    specs: &[SourceSpec],
    watched: &[PathBuf],
    watching: bool,
    tx: mpsc::UnboundedSender<()>,
) -> Option<(notify::RecommendedWatcher, Vec<PathBuf>)> {
    use notify::Watcher as _;
    let (dirs, relevant) = watch_plan(specs);
    if dirs == watched && watching {
        return None;
    }
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
            Some((watcher, dirs))
        }
        Err(err) => {
            tracing::warn!("kubeconfig hot reload is off: {err}");
            None
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::kubeconfig::fixtures::{DEV, OTHER};
    use kubyl_base::host::TestHost;

    fn setup() -> (tempfile::TempDir, TestHost<ManagerCore>, ManagerCore) {
        let dir = tempfile::tempdir().unwrap();
        let kube = dir.path().join("kube");
        std::fs::create_dir_all(&kube).unwrap();
        std::fs::write(kube.join("dev.yaml"), DEV).unwrap();
        std::fs::write(kube.join("other.yaml"), OTHER).unwrap();
        let settings = KubeSettings {
            load_default_kubeconfig: false,
            load_kubeconfig_env: false,
            kubeconfigs: vec![
                kube.join("dev.yaml").display().to_string(),
                kube.join("other.yaml").display().to_string(),
            ],
            ..Default::default()
        };
        let mut host = TestHost::new();
        let mut core = ManagerCore::new(
            dir.path().join("pasted"),
            false,
            settings,
            KubeState::default(),
        );
        core.start(&mut host);
        host.run_until_idle(&mut core);
        (dir, host, core)
    }

    #[test]
    fn loads_without_a_ui() {
        let (_dir, host, core) = setup();
        let names: Vec<_> = core.contexts().map(|c| c.name.clone()).collect();
        assert_eq!(names, ["kind-dev", "prod", "prod@other", "broken"]);
        assert!(host.events.contains(&ConnectionEvent::ContextsChanged));
        assert!(
            host.effects
                .iter()
                .any(|e| matches!(e, KubeEffect::ClusterIdsChanged))
        );
    }

    #[test]
    fn stale_reload_results_are_ignored() {
        let (_dir, mut host, mut core) = setup();
        let before = core.contexts().count();
        assert!(before > 0);
        core.reload_generation += 1;
        let generation = core.reload_generation;
        // A result of an older reload arrives after a newer one started.
        core.finish_reload(generation - 1, Loaded::default(), &[], &mut host);
        assert_eq!(core.contexts().count(), before);
        core.finish_reload(generation, Loaded::default(), &[], &mut host);
        assert_eq!(core.contexts().count(), 0);
    }

    #[test]
    fn broken_contexts_fail_and_activate_without_network() {
        let (_dir, mut host, mut core) = setup();
        let broken = core.all_contexts()[3].id.clone();
        core.activate(&broken, &mut host);
        host.run_until_idle(&mut core);
        assert!(matches!(
            core.state(&broken),
            ConnectionState::Unreachable { retry_in: None, .. }
        ));
        assert_eq!(core.active(), Some(&broken));
        assert!(host.effects.iter().any(|e| matches!(
            e,
            KubeEffect::SaveState(KubeState { active: Some(id), .. }) if id == &broken
        )));
        assert!(!core.badge_info(&broken).connected);
    }

    #[test]
    fn settings_changes_are_asked_of_the_host() {
        let (_dir, mut host, mut core) = setup();
        let prod = core.contexts().nth(1).unwrap().id.clone();
        core.update_context_settings(&prod, &mut host, |s| s.production = true);
        let Some(KubeEffect::UpdateSettings(update)) = host.effects.pop() else {
            panic!("expected a settings update");
        };
        let mut settings = core.settings().clone();
        update(&mut settings);
        core.settings_changed(settings, &mut host);
        assert!(core.caps(&prod).production);
    }

    #[test]
    fn watch_plan_covers_parents_and_folder_sources() {
        let dir = tempfile::tempdir().unwrap();
        let file = dir.path().join("kube/config");
        std::fs::create_dir_all(file.parent().unwrap()).unwrap();
        let folder = dir.path().join("pasted");
        let spec = |path: &Path, files: Vec<PathBuf>, is_dir| SourceSpec {
            kind: crate::kubeconfig::SourceKind::Env,
            path: path.to_path_buf(),
            files,
            is_dir,
        };
        let (dirs, relevant) = watch_plan(&[
            spec(&file, vec![file.clone()], false),
            spec(&folder, Vec::new(), true),
        ]);
        // The folder source is created so it can be watched.
        assert_eq!(
            dirs,
            vec![file.parent().unwrap().to_path_buf(), folder.clone()]
        );
        assert!(relevant.contains(&file) && relevant.contains(&folder));
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
