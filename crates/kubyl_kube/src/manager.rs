//! [`ConnectionManager`]: the app-wide entity around [`ManagerCore`], which owns kubeconfig
//! sources, contexts and one connection per cluster.
//!
//! Reads go straight to the core (the entity derefs to it). Changes go through the methods
//! here, which run the core on a [`kubyl_core::host::GpuiHost`] and apply what it asks for:
//! state.json, settings.json, the title bar's [`ActiveContext`] and [`ClusterIds`].

use std::ops::Deref;
use std::path::{Path, PathBuf};

use gpui::{
    App, AppContext as _, Context, Entity, EventEmitter, Global, Hsla, SharedString, Subscription,
    Task,
};
use kubyl_core::host::{Hosts, hosted};
use kubyl_core::{ActiveContext, ClusterBadge, ClusterId, ClusterIds, TabContext, spawn_kube};
pub use kubyl_kube_core::manager::{
    BadgeInfo, Cluster, ConnectionEvent, ConnectionState, KubeEffect, KubeState, ManagerCore,
    Namespaces,
};
use kubyl_settings::{Settings, State};
use kubyl_ui::ActiveColors as _;

use crate::access::{self, AccessQuery};
use crate::kubeconfig;
use crate::settings::{ColorTagExt as _, ContextSettings, KubeSettings};

/// Theme colors of connection states.
pub trait ConnectionStateExt {
    /// Theme color for status dots.
    fn color(&self, colors: &kubyl_ui::Colors) -> Hsla;
}

impl ConnectionStateExt for ConnectionState {
    fn color(&self, colors: &kubyl_ui::Colors) -> Hsla {
        match self {
            ConnectionState::Disconnected => colors.text_faint,
            ConnectionState::Connecting => colors.yellow,
            ConnectionState::Connected { .. } => colors.green,
            ConnectionState::AuthRequired { .. } => colors.yellow,
            ConnectionState::Unreachable { .. } | ConnectionState::Forbidden(_) => colors.red,
        }
    }
}

/// The app's [`ManagerCore`]. There is one per app: [`ConnectionManager::global`]. See the
/// crate docs for how the sidebar and other crates use it.
pub struct ConnectionManager {
    core: ManagerCore,
    _subscriptions: Vec<Subscription>,
}

impl EventEmitter<ConnectionEvent> for ConnectionManager {}

impl Deref for ConnectionManager {
    type Target = ManagerCore;

    fn deref(&self) -> &ManagerCore {
        &self.core
    }
}

impl Hosts<ManagerCore> for ConnectionManager {
    fn service(&mut self) -> &mut ManagerCore {
        &mut self.core
    }

    fn apply(&mut self, effect: KubeEffect, cx: &mut Context<Self>) {
        match effect {
            KubeEffect::SaveState(state) => State::set(cx, &state),
            KubeEffect::UpdateSettings(update) => Settings::update::<KubeSettings>(cx, update),
            KubeEffect::SyncActive => self.sync_active(cx),
            KubeEffect::Activated {
                id,
                namespace,
                from_tab,
            } => {
                let context = ActiveContext {
                    cluster: Some(self.badge(&id, cx)),
                    namespace: namespace.map(SharedString::from),
                };
                if from_tab {
                    ActiveContext::set_from_tab(cx, context);
                } else {
                    ActiveContext::set(cx, context);
                }
            }
            KubeEffect::ClearActive => ActiveContext::set(cx, ActiveContext::default()),
            KubeEffect::ClusterIdsChanged => ClusterIds::changed(cx),
        }
    }
}

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

    /// Installs the global (`watch_files: false` in GPUI tests of other crates).
    pub fn install(pasted_dir: PathBuf, watch_files: bool, cx: &mut App) -> Entity<Self> {
        let manager = cx.new(|cx| Self::new(pasted_dir, watch_files, cx));
        cx.set_global(GlobalManager(manager.clone()));
        let weak = manager.downgrade();
        ClusterIds::install(cx, move |id, cx| {
            weak.upgrade()
                .map(|m| m.read(cx).resolve(id))
                .unwrap_or_else(|| id.clone())
        });
        manager.update(cx, |m, cx| hosted(m, cx, |core, host| core.start(host)));
        manager
    }

    fn new(pasted_dir: PathBuf, watch_files: bool, cx: &mut Context<Self>) -> Self {
        let weak = cx.weak_entity();
        let subscription = Settings::observe::<KubeSettings>(cx, move |settings, cx| {
            let settings = settings.clone();
            weak.update(cx, |this, cx| {
                hosted(this, cx, |core, host| core.settings_changed(settings, host))
            })
            .ok();
        });
        let namespaces = cx.observe_global::<ActiveContext>(|this, cx| this.remember_namespace(cx));
        Self {
            core: ManagerCore::new(
                pasted_dir,
                watch_files,
                Settings::get::<KubeSettings>(cx).clone(),
                State::get::<KubeState>(cx),
            ),
            _subscriptions: vec![subscription, namespaces],
        }
    }

    // ----- Sources and contexts -----

    /// Display name: the override, or the entry's label.
    pub fn display_name(&self, id: &ClusterId) -> SharedString {
        self.core.display_name(id).into()
    }

    pub fn color(&self, id: &ClusterId, cx: &App) -> Hsla {
        self.core.color_tag(id).color(cx.colors())
    }

    /// Re-reads every source.
    pub fn reload(&mut self, cx: &mut Context<Self>) {
        hosted(self, cx, |core, host| core.reload(host));
    }

    /// Adds kubeconfig files or folders as sources (stored in settings.json).
    pub fn add_sources(&mut self, paths: Vec<PathBuf>, cx: &mut Context<Self>) {
        hosted(self, cx, |core, host| core.add_sources(paths, host));
    }

    /// Removes a user-added source. Files are never deleted, except pasted ones.
    pub fn remove_source(&mut self, path: &Path, cx: &mut Context<Self>) {
        hosted(self, cx, |core, host| core.remove_source(path, host));
    }

    /// Turns loading `~/.kube/config` on or off (settings.json). The file itself is untouched.
    pub fn set_load_default_kubeconfig(&mut self, load: bool, cx: &mut Context<Self>) {
        hosted(self, cx, |core, host| {
            core.set_load_default_kubeconfig(load, host)
        });
    }

    /// Turns loading the files in `$KUBECONFIG` on or off (settings.json).
    pub fn set_load_kubeconfig_env(&mut self, load: bool, cx: &mut Context<Self>) {
        hosted(self, cx, |core, host| {
            core.set_load_kubeconfig_env(load, host)
        });
    }

    /// Deletes a pasted kubeconfig: the copy Kubyl keeps in [`ManagerCore::pasted_dir`].
    /// Refuses any other path, since Kubyl never deletes the user's own kubeconfig files.
    pub fn delete_pasted(
        &mut self,
        path: &Path,
        cx: &mut Context<Self>,
    ) -> Task<Result<(), String>> {
        if let Err(err) = self.core.check_pasted(path) {
            return Task::ready(Err(err));
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
        let dir = self.core.pasted_dir().to_path_buf();
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

    /// Changes an entry's overrides in settings.json (see
    /// [`ManagerCore::update_context_settings`]).
    pub fn update_context_settings(
        &mut self,
        id: &ClusterId,
        cx: &mut Context<Self>,
        f: impl FnOnce(&mut ContextSettings),
    ) {
        hosted(self, cx, |core, host| {
            core.update_context_settings(id, host, f)
        });
    }

    /// Turns grouping of contexts on or off (settings.json `kubernetes.group_contexts`).
    pub fn set_group_contexts(&mut self, group: bool, cx: &mut Context<Self>) {
        hosted(self, cx, |core, host| core.set_group_contexts(group, host));
    }

    /// Shows a group's contexts as separate entries, or (`separate: false`) as one again.
    pub fn set_group_separate(
        &mut self,
        group: &ClusterId,
        separate: bool,
        cx: &mut Context<Self>,
    ) {
        hosted(self, cx, |core, host| {
            core.set_group_separate(group, separate, host)
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
        hosted(self, cx, |core, host| {
            core.move_context_settings(from, to, host)
        });
    }

    // ----- Connections -----

    /// Open API server connections of each cluster that has a client, by display name.
    pub fn open_connections(&self) -> Vec<(SharedString, crate::transport::OpenConnections)> {
        self.core
            .open_connections()
            .into_iter()
            .map(|(name, open)| (name.into(), open))
            .collect()
    }

    /// Connects unless already connected or connecting. Use when a cluster root is expanded or
    /// a favorite is opened.
    pub fn ensure_connected(&mut self, id: &ClusterId, cx: &mut Context<Self>) {
        hosted(self, cx, |core, host| core.ensure_connected(id, host));
    }

    /// (Re)connects: builds a client, checks it, then starts discovery, watches and the health
    /// ping.
    pub fn connect(&mut self, id: &ClusterId, cx: &mut Context<Self>) {
        hosted(self, cx, |core, host| core.connect(id, host));
    }

    /// Disconnects and forgets the client (watches stop).
    pub fn disconnect(&mut self, id: &ClusterId, cx: &mut Context<Self>) {
        hosted(self, cx, |core, host| core.disconnect(id, host));
    }

    /// Connects on the user's behalf: a context that needs a sign-in opens the modal.
    pub fn connect_interactive(&mut self, id: &ClusterId, cx: &mut Context<Self>) {
        hosted(self, cx, |core, host| core.connect_interactive(id, host));
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
        if let Some(answer) = self.core.cached_can_i(id, &query) {
            return Task::ready(Some(answer));
        }
        let Some((client, generation)) = self.core.access_check(id) else {
            return Task::ready(None);
        };
        let check = spawn_kube(cx, access::check(client, query.clone()));
        let id = id.clone();
        cx.spawn(async move |this, cx| {
            let allowed = check
                .await
                .inspect_err(|err| tracing::debug!("access check failed: {err}"))
                .ok()?;
            this.update(cx, |this, _| {
                this.core.record_access(&id, generation, query, allowed)
            })
            .ok();
            Some(allowed)
        })
    }

    /// The OpenAPI v3 spec of a group-version (`OpenApiIndex::key(group, version)`), from the
    /// on-disk cache when unchanged.
    pub fn openapi_spec(
        &self,
        id: &ClusterId,
        path: String,
        cx: &mut Context<Self>,
    ) -> Task<Result<serde_json::Value, String>> {
        match self.core.openapi_request(id, path) {
            Ok(request) => {
                let task = spawn_kube(cx, request);
                cx.background_executor().spawn(task)
            }
            Err(err) => Task::ready(Err(err)),
        }
    }

    // ----- Active cluster -----

    /// Makes `id` the active cluster (title bar, status bar) and connects it.
    pub fn activate(&mut self, id: &ClusterId, cx: &mut Context<Self>) {
        hosted(self, cx, |core, host| core.activate(id, host));
    }

    /// Shows the context of an activated tab in the title bar: its cluster, and its namespace
    /// unless the tab leaves that to the title bar. Views in other tabs keep their namespace.
    pub fn follow_tab(&mut self, context: &TabContext, cx: &mut Context<Self>) {
        let shown = ActiveContext::global(cx);
        let cluster = shown.cluster.as_ref().map(|c| c.id.clone());
        let namespace = shown.namespace.as_ref().map(|n| n.to_string());
        hosted(self, cx, |core, host| {
            core.follow_tab(context, (cluster.as_ref(), namespace.as_deref()), host)
        });
    }

    /// Keeps the last namespace used in each group (state.json).
    fn remember_namespace(&mut self, cx: &mut Context<Self>) {
        let active = ActiveContext::global(cx);
        let (Some(cluster), Some(namespace)) = (&active.cluster, &active.namespace) else {
            return;
        };
        let id = cluster.id.clone();
        let namespace = namespace.to_string();
        hosted(self, cx, |core, host| {
            core.remember_namespace(&id, &namespace, host)
        });
    }

    /// How the chrome shows a cluster.
    pub fn badge(&self, id: &ClusterId, cx: &App) -> ClusterBadge {
        let info = self.core.badge_info(id);
        ClusterBadge {
            id: info.id,
            name: info.name.into(),
            color: info.color.color(cx.colors()),
            production: info.production,
            meta: info.meta.map(SharedString::from),
            connected: info.connected,
        }
    }

    /// Keeps the title/status bar badge in sync, preserving the selected namespace.
    fn sync_active(&self, cx: &mut Context<Self>) {
        let Some(id) = self.core.active() else {
            return;
        };
        let badge = self.badge(id, cx);
        let current = ActiveContext::global(cx).clone();
        if current.cluster.as_ref() != Some(&badge) {
            let context = ActiveContext {
                cluster: Some(badge),
                namespace: current.namespace,
            };
            // Only the badge changed: views that ignored a tab's namespace keep ignoring it.
            if ActiveContext::follows_tab(cx) {
                ActiveContext::set_from_tab(cx, context);
            } else {
                ActiveContext::set(cx, context);
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::kubeconfig::fixtures::{DEV, OTHER};
    use gpui::TestAppContext;
    use kubyl_core::TabNamespace;
    use kubyl_kube_core::manager::fallback_namespaces;

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
    fn safety_flags_survive_a_disconnect(cx: &mut TestAppContext) {
        let (_dir, manager) = setup(cx);
        let broken = manager.read_with(cx, |m, _| m.all_contexts()[3].id.clone());
        manager.update(cx, |m, cx| {
            m.update_context_settings(&broken, cx, |s| {
                s.production = true;
                s.read_only = true;
            });
            m.activate(&broken, cx);
        });
        cx.run_until_parked();
        manager.update(cx, |m, cx| m.disconnect(&broken, cx));
        cx.run_until_parked();
        manager.read_with(cx, |m, _| {
            assert!(m.cluster(&broken).is_some(), "the connection entry exists");
            let caps = m.caps(&broken);
            assert!(caps.production && caps.read_only, "{caps:?}");
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
    fn activated_tabs_show_their_namespace(cx: &mut TestAppContext) {
        let (_dir, manager) = setup(cx);
        // The only context that fails without touching the network.
        let broken = manager.read_with(cx, |m, _| m.all_contexts()[3].id.clone());
        manager.update(cx, |m, cx| m.activate(&broken, cx));
        cx.run_until_parked();
        let namespace = |cx: &mut TestAppContext| {
            cx.update(|cx| {
                let active = ActiveContext::global(cx);
                (
                    active.namespace.as_ref().map(|n| n.to_string()),
                    ActiveContext::follows_tab(cx),
                )
            })
        };
        assert_eq!(namespace(cx), (None, false));
        let follow = |namespace: TabNamespace, cx: &mut TestAppContext| {
            let context = TabContext {
                cluster: broken.clone(),
                namespace,
            };
            manager.update(cx, |m, cx| m.follow_tab(&context, cx));
        };
        follow(TabNamespace::One("payments".into()), cx);
        assert_eq!(namespace(cx), (Some("payments".into()), true));
        // A view that follows the title bar leaves it alone.
        follow(TabNamespace::Keep, cx);
        assert_eq!(namespace(cx), (Some("payments".into()), true));
        follow(TabNamespace::All, cx);
        assert_eq!(namespace(cx), (None, true));
        manager.update(cx, |m, cx| m.activate(&broken, cx));
        assert_eq!(namespace(cx), (None, false));
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
            m.core.fake_connected(&single);
            m.core.set_active_for_tests(Some(single.clone()));
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
            assert_eq!(cluster.generation(), 42);
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
            assert_eq!(m.cluster(&group).unwrap().generation(), 42);
        });

        // The siblings go away again: back to the single context's id, still connected.
        std::fs::write(&file, oc_config(&["shop"], "shop")).unwrap();
        manager.update(cx, |m, cx| m.reload(cx));
        cx.run_until_parked();
        manager.read_with(cx, |m, _| {
            let entry = m.contexts().next().unwrap();
            assert_eq!(entry.id, single);
            assert_eq!(m.cluster(&single).unwrap().generation(), 42);
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
                crate::settings::ColorTagExt::color(
                    crate::settings::ColorTag::Green,
                    kubyl_ui::ActiveColors::colors(cx)
                )
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
            assert!(!m.settings().context(shop.as_str()).production);
            // The color stays (written under the group id).
            assert!(m.settings().context(group.as_str()).color.is_some());
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
}
