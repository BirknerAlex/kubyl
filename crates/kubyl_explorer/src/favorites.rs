//! Favorites: namespaces (optionally a kind and label selector) from any cluster, pinned at the
//! top of the sidebar and stored in state.json.
//!
//! A favorite is keyed by **context name + API server URL**, with the kubeconfig file as a
//! hint. [`resolve`] finds the context again after a kubeconfig reload, an edit, or a moved
//! file:
//! 1. same context name, server and file;
//! 2. same context name and server, any file (the file moved or was copied);
//! 3. same context name and file (the server URL was edited);
//! 4. same server and file (the context was renamed).
//!
//! `ClusterId` (`<context>@<path>`) isn't stored because it changes when the file moves.

use std::path::PathBuf;

use gpui::{App, AppContext as _, Context, Entity, EventEmitter, Global};
use kubyl_core::{ClusterId, Gvr};
use kubyl_kube::kubeconfig::ContextInfo;
use kubyl_settings::{State, StateSection};
use serde::{Deserialize, Serialize};

/// One favorite.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Favorite {
    /// Context name inside its kubeconfig.
    pub context: String,
    /// API server URL of the context's cluster.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub server: Option<String>,
    /// The kubeconfig file the context came from.
    pub file: PathBuf,
    pub namespace: String,
    /// Resource to open (`pods`, `deployments.apps`). `None`: pods.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub kind: Option<String>,
    /// Label selector applied to the list.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub selector: Option<String>,
    /// Shown instead of the namespace.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub alias: Option<String>,
}

impl Favorite {
    pub fn new(context: &ContextInfo, namespace: impl Into<String>) -> Self {
        Self {
            context: context.context.clone(),
            server: context.server.clone(),
            file: context.file.clone(),
            namespace: namespace.into(),
            kind: None,
            selector: None,
            alias: None,
        }
    }

    /// The label in the sidebar: the alias, or the namespace.
    pub fn label(&self) -> &str {
        self.alias.as_deref().unwrap_or(&self.namespace)
    }

    /// Same place (context, server, file, namespace, kind and selector), ignoring the alias.
    pub fn same_target(&self, other: &Favorite) -> bool {
        self.context == other.context
            && self.server == other.server
            && self.file == other.file
            && self.namespace == other.namespace
            && self.kind == other.kind
            && self.selector == other.selector
    }

    /// The resource to list: `kind` split into plural and group (`deployments.apps`).
    pub fn gvr(&self) -> Gvr {
        let kind = self.kind.as_deref().unwrap_or("pods");
        match kind.split_once('.') {
            Some((resource, group)) => Gvr::new(group, "v1", resource),
            None => Gvr::new("", "v1", kind),
        }
    }
}

/// Finds the context a favorite points at (see the module docs for the rules).
pub fn resolve<'a>(favorite: &Favorite, contexts: &'a [ContextInfo]) -> Option<&'a ContextInfo> {
    let rules: [&dyn Fn(&ContextInfo) -> bool; 4] = [
        &|c| {
            c.context == favorite.context && c.server == favorite.server && c.file == favorite.file
        },
        &|c| c.context == favorite.context && c.server == favorite.server,
        &|c| c.context == favorite.context && c.file == favorite.file,
        &|c| favorite.server.is_some() && c.server == favorite.server && c.file == favorite.file,
    ];
    rules
        .iter()
        .find_map(|rule| contexts.iter().find(|c| rule(c)))
}

/// What state.json stores under `favorites`.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct FavoritesState {
    pub items: Vec<Favorite>,
    /// Favorites left out of the Favorites workspace (indices into `items`).
    pub workspace_excluded: Vec<usize>,
}

impl StateSection for FavoritesState {
    const KEY: &'static str = "favorites";
}

/// Emitted whenever the list changes.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct FavoritesChanged;

/// The app's favorites. Views observe [`Favorites::global`].
pub struct Favorites {
    state: FavoritesState,
}

impl EventEmitter<FavoritesChanged> for Favorites {}

struct GlobalFavorites(Entity<Favorites>);

impl Global for GlobalFavorites {}

impl Favorites {
    pub fn global(cx: &App) -> Entity<Self> {
        cx.global::<GlobalFavorites>().0.clone()
    }

    pub(crate) fn install(cx: &mut App) {
        let state = State::get::<FavoritesState>(cx);
        let entity = cx.new(|_| Self { state });
        cx.set_global(GlobalFavorites(entity));
    }

    pub fn items(&self) -> &[Favorite] {
        &self.state.items
    }

    pub fn is_excluded_from_workspace(&self, index: usize) -> bool {
        self.state.workspace_excluded.contains(&index)
    }

    fn changed(&mut self, cx: &mut Context<Self>) {
        State::set(cx, &self.state);
        cx.emit(FavoritesChanged);
        cx.notify();
    }

    pub fn position(&self, favorite: &Favorite) -> Option<usize> {
        self.state
            .items
            .iter()
            .position(|f| f.same_target(favorite))
    }

    /// Adds a favorite unless the same one exists. Returns whether it was added.
    pub fn add(&mut self, favorite: Favorite, cx: &mut Context<Self>) -> bool {
        if self.position(&favorite).is_some() {
            return false;
        }
        self.state.items.push(favorite);
        self.changed(cx);
        true
    }

    /// Adds or removes a favorite. Returns whether it's a favorite now.
    pub fn toggle(&mut self, favorite: Favorite, cx: &mut Context<Self>) -> bool {
        match self.position(&favorite) {
            Some(index) => {
                self.remove(index, cx);
                false
            }
            None => self.add(favorite, cx),
        }
    }

    pub fn remove(&mut self, index: usize, cx: &mut Context<Self>) {
        if index < self.state.items.len() {
            self.state.items.remove(index);
            self.state.workspace_excluded = self
                .state
                .workspace_excluded
                .iter()
                .filter(|&&i| i != index)
                .map(|&i| if i > index { i - 1 } else { i })
                .collect();
            self.changed(cx);
        }
    }

    /// Moves the favorite at `from` so it ends up at `to` (drag to reorder).
    pub fn move_item(&mut self, from: usize, to: usize, cx: &mut Context<Self>) {
        let len = self.state.items.len();
        if from >= len || to >= len || from == to {
            return;
        }
        let item = self.state.items.remove(from);
        self.state.items.insert(to, item);
        // Keep workspace exclusions attached to the same favorites.
        let mut order: Vec<usize> = (0..len).collect();
        let moved = order.remove(from);
        order.insert(to, moved);
        self.state.workspace_excluded = self
            .state
            .workspace_excluded
            .iter()
            .filter_map(|old| order.iter().position(|o| o == old))
            .collect();
        self.changed(cx);
    }

    pub fn rename(&mut self, index: usize, alias: Option<String>, cx: &mut Context<Self>) {
        if let Some(item) = self.state.items.get_mut(index) {
            item.alias = alias.filter(|a| !a.trim().is_empty());
            self.changed(cx);
        }
    }

    pub fn set_in_workspace(&mut self, index: usize, included: bool, cx: &mut Context<Self>) {
        self.state.workspace_excluded.retain(|&i| i != index);
        if !included {
            self.state.workspace_excluded.push(index);
        }
        self.changed(cx);
    }

    /// The favorite of `namespace` on the cluster entry `cluster` (saved with any of its
    /// contexts), without a kind or selector: the namespace switcher's star.
    pub fn namespace_position(
        &self,
        cluster: &ClusterId,
        namespace: &str,
        cx: &App,
    ) -> Option<usize> {
        self.state.items.iter().position(|f| {
            f.namespace == namespace
                && f.kind.is_none()
                && f.selector.is_none()
                && cluster_of(f, cx).as_ref() == Some(cluster)
        })
    }

    /// Adds or removes the namespace favorite of a cluster entry. Returns whether it's a
    /// favorite now.
    pub fn toggle_namespace(
        &mut self,
        context: &ContextInfo,
        namespace: &str,
        cx: &mut Context<Self>,
    ) -> bool {
        match self.namespace_position(&context.id, namespace, cx) {
            Some(index) => {
                self.remove(index, cx);
                false
            }
            None => self.add(Favorite::new(context, namespace), cx),
        }
    }
}

/// The cluster entry a favorite resolves to right now (the group of a grouped context).
pub fn cluster_of(favorite: &Favorite, cx: &App) -> Option<ClusterId> {
    let manager = kubyl_kube::ConnectionManager::try_global(cx)?;
    let manager = manager.read(cx);
    resolve(favorite, manager.all_contexts()).map(|c| manager.resolve(&c.id))
}

#[cfg(test)]
mod tests {
    use super::*;
    use kubyl_kube::auth::AuthMethod;
    use kubyl_kube::kubeconfig::{CaSource, SourceKind};

    fn context(name: &str, server: &str, file: &str) -> ContextInfo {
        ContextInfo {
            id: ContextInfo::make_id(name, std::path::Path::new(file)),
            name: name.into(),
            context: name.into(),
            file: file.into(),
            source: SourceKind::User,
            source_path: file.into(),
            cluster: name.into(),
            user: None,
            server: Some(server.into()),
            namespace: None,
            auth: AuthMethod::None,
            insecure_skip_tls_verify: false,
            proxy_url: None,
            tls_server_name: None,
            ca: CaSource::System,
            error: None,
            members: Vec::new(),
            group: None,
        }
    }

    #[test]
    fn resolves_after_moves_and_edits() {
        let original = context("prod", "https://prod:6443", "/a/config");
        let favorite = Favorite::new(&original, "payments");
        assert_eq!(
            resolve(&favorite, std::slice::from_ref(&original))
                .unwrap()
                .id,
            original.id
        );

        // The file moved.
        let moved = context("prod", "https://prod:6443", "/b/config");
        assert_eq!(
            resolve(&favorite, std::slice::from_ref(&moved)).unwrap().id,
            moved.id
        );

        // The server URL changed in place.
        let edited = context("prod", "https://prod-new:6443", "/a/config");
        assert_eq!(
            resolve(&favorite, std::slice::from_ref(&edited))
                .unwrap()
                .id,
            edited.id
        );

        // The context was renamed in place.
        let renamed = context("production", "https://prod:6443", "/a/config");
        assert_eq!(
            resolve(&favorite, std::slice::from_ref(&renamed))
                .unwrap()
                .id,
            renamed.id
        );

        // An exact match wins over a looser one.
        let both = [moved.clone(), original.clone()];
        assert_eq!(resolve(&favorite, &both).unwrap().id, original.id);

        let unrelated = context("staging", "https://staging:6443", "/c/config");
        assert!(resolve(&favorite, &[unrelated]).is_none());
    }

    #[test]
    fn kinds_map_to_gvrs() {
        let mut favorite = Favorite::new(&context("c", "s", "/f"), "ns");
        assert_eq!(favorite.gvr(), Gvr::new("", "v1", "pods"));
        favorite.kind = Some("deployments.apps".into());
        assert_eq!(favorite.gvr(), Gvr::new("apps", "v1", "deployments"));
    }

    #[gpui::test]
    fn add_toggle_move_and_rename(cx: &mut gpui::TestAppContext) {
        let dir = tempfile::tempdir().unwrap();
        cx.update(|cx| {
            kubyl_settings::init_with_dir(cx, dir.path());
            Favorites::install(cx);
        });
        let favorites = cx.update(|cx| Favorites::global(cx));
        let a = Favorite::new(&context("a", "s1", "/f1"), "payments");
        let b = Favorite::new(&context("b", "s2", "/f2"), "payments");
        let c = Favorite::new(&context("c", "s3", "/f3"), "checkout");
        favorites.update(cx, |f, cx| {
            assert!(f.add(a.clone(), cx));
            assert!(!f.add(a.clone(), cx));
            f.add(b.clone(), cx);
            f.add(c.clone(), cx);
            f.set_in_workspace(2, false, cx);
            f.move_item(2, 0, cx);
            assert_eq!(f.items()[0].context, "c");
            assert!(f.is_excluded_from_workspace(0));
            f.rename(1, Some("pay".into()), cx);
            assert_eq!(f.items()[1].label(), "pay");
            assert!(!f.toggle(b.clone(), cx));
            assert_eq!(f.items().len(), 2);
        });
        cx.update(|cx| {
            let saved = State::get::<FavoritesState>(cx);
            assert_eq!(saved.items.len(), 2);
            assert_eq!(saved.workspace_excluded, [0]);
        });
    }
}
