//! Favorites: namespaces (optionally a kind and label selector) or any view (a tab's
//! [`ViewRequest`]) from any cluster, pinned at the top of the sidebar and stored in state.json.
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
use kubyl_core::{ActiveContext, ClusterId, Gvr, TabHandle, TabNamespace, ViewKind, ViewRequest};
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
    /// `None`: the view isn't scoped to a namespace (all namespaces, a cluster-level view).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub namespace: Option<String>,
    /// The view shows all namespaces (and reopens that way), as opposed to a view that isn't
    /// scoped to a namespace at all.
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub all_namespaces: bool,
    /// The view to open, for a favorite made from a tab. Its target's cluster is blank (a
    /// cluster id changes when the kubeconfig moves); [`Favorite::request`] fills it in.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub view: Option<ViewRequest>,
    /// Tab title of a view favorite, shown unless there's an alias.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub title: Option<String>,
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
            namespace: Some(namespace.into()),
            all_namespaces: false,
            view: None,
            title: None,
            kind: None,
            selector: None,
            alias: None,
        }
    }

    /// A favorite of the view `request` (a tab) in `namespace` of `context`.
    pub fn for_view(
        context: &ContextInfo,
        scope: Scope,
        title: impl Into<String>,
        mut request: ViewRequest,
    ) -> Self {
        if let Some(target) = &mut request.target {
            target.cluster = ClusterId::new("");
        }
        Self {
            namespace: scope.namespace,
            all_namespaces: scope.all,
            view: Some(request),
            title: Some(title.into()),
            ..Self::new(context, "")
        }
    }

    /// The label in the sidebar: the alias, the view's title, or the namespace.
    pub fn label(&self) -> &str {
        self.alias
            .as_deref()
            .or(self.title.as_deref())
            .or(self.namespace.as_deref())
            .unwrap_or(&self.context)
    }

    /// The request that opens a view favorite on `cluster`.
    pub fn request(&self, cluster: &ClusterId) -> Option<ViewRequest> {
        let mut request = self.view.clone()?;
        if let Some(target) = &mut request.target {
            target.cluster = cluster.clone();
        }
        Some(request)
    }

    /// Same place (context, server, file, namespace, kind and selector), ignoring the alias.
    pub fn same_target(&self, other: &Favorite) -> bool {
        self.context == other.context
            && self.server == other.server
            && self.file == other.file
            && self.same_view(other)
    }

    /// Same namespace, view, kind and selector, in whichever cluster.
    fn same_view(&self, other: &Favorite) -> bool {
        self.namespace == other.namespace
            && self.all_namespaces == other.all_namespaces
            && self.view == other.view
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

/// The namespace a favorite's view is scoped to.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Scope {
    pub namespace: Option<String>,
    /// All namespaces, explicitly.
    pub all: bool,
}

impl Scope {
    /// From what a tab reports: its [`TabNamespace`], the namespace of its request's target
    /// and the title bar's namespace (for views that follow it).
    fn of_tab(
        tab: Option<TabNamespace>,
        target: Option<Option<String>>,
        active: Option<String>,
    ) -> Self {
        match tab {
            Some(TabNamespace::One(namespace)) => Self {
                namespace: Some(namespace),
                all: false,
            },
            Some(TabNamespace::All) => Self {
                namespace: None,
                all: true,
            },
            // The view follows the title bar's namespace.
            Some(TabNamespace::Keep) | None => Self {
                namespace: target.unwrap_or(active),
                all: false,
            },
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

    /// Like [`Self::global`], before the explorer is initialized (tests of other crates).
    pub fn try_global(cx: &App) -> Option<Entity<Self>> {
        cx.try_global::<GlobalFavorites>().map(|g| g.0.clone())
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

    /// Where `favorite` is saved: the same place, or the same view, namespace and kind in the
    /// cluster entry it resolves to (the kubeconfig moved, the context was renamed).
    pub fn position(&self, favorite: &Favorite, cx: &App) -> Option<usize> {
        let cluster = cluster_of(favorite, cx);
        self.state.items.iter().position(|f| {
            f.same_target(favorite)
                || (cluster.is_some() && f.same_view(favorite) && cluster_of(f, cx) == cluster)
        })
    }

    /// Adds a favorite unless the same one exists. Returns whether it was added.
    pub fn add(&mut self, favorite: Favorite, cx: &mut Context<Self>) -> bool {
        if self.position(&favorite, cx).is_some() {
            return false;
        }
        self.state.items.push(favorite);
        self.changed(cx);
        true
    }

    /// Adds or removes a favorite. Returns whether it's a favorite now.
    pub fn toggle(&mut self, favorite: Favorite, cx: &mut Context<Self>) -> bool {
        match self.position(&favorite, cx) {
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
            f.namespace.as_deref() == Some(namespace)
                && f.view.is_none()
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

/// The favorite that opens `tab` again: its view request, in the cluster and namespace the tab
/// shows. `None` for tabs that can't be restored, have no cluster (the welcome tab, a
/// kubeconfig file) or aren't worth pinning.
pub fn from_tab(tab: &dyn TabHandle, cx: &App) -> Option<Favorite> {
    let request = tab.view_request(cx)?;
    // Not worth pinning: the welcome tab and settings, local files, and sessions that would
    // start a shell or a file transfer on a pod name that goes stale.
    if matches!(
        request.kind,
        ViewKind::Welcome | ViewKind::Settings | ViewKind::Terminal | ViewKind::Files
    ) || request.path.is_some()
        || request.kind == crate::favorites_view_kind()
    {
        return None;
    }
    let context = tab.tab_context(cx);
    let active = ActiveContext::global(cx);
    let cluster = context
        .as_ref()
        .map(|c| c.cluster.clone())
        .or_else(|| request.target.as_ref().map(|t| t.cluster.clone()))
        .filter(|c| !c.as_str().is_empty())
        .or_else(|| active.cluster.as_ref().map(|c| c.id.clone()))?;
    let manager = kubyl_kube::ConnectionManager::try_global(cx)?;
    let info = manager.read(cx).context(&cluster)?.clone();
    // A cluster-scoped kind (Nodes) has no namespace, whatever the title bar shows.
    let cluster_scoped = request.target.as_ref().is_some_and(|target| {
        manager
            .read(cx)
            .discovery(&cluster)
            .and_then(|d| d.by_gvr(&target.gvr).map(|r| !r.namespaced))
            .unwrap_or(false)
    });
    let scope = if cluster_scoped {
        Scope::default()
    } else {
        Scope::of_tab(
            context.map(|c| c.namespace),
            request.target.as_ref().map(|t| t.namespace.clone()),
            active.namespace.as_ref().map(|n| n.to_string()),
        )
    };
    Some(Favorite::for_view(
        &info,
        scope,
        tab.title(cx).to_string(),
        request,
    ))
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
    fn view_favorites_keep_their_request_without_a_cluster() {
        use kubyl_core::ResourceRef;
        let info = context("c", "s", "/f");
        let request = ViewRequest::for_resource(
            ViewKind::Table,
            ResourceRef::list(
                info.id.clone(),
                Gvr::new("apps", "v1", "deployments"),
                Some("ns".into()),
            ),
        );
        let scope = Scope {
            namespace: Some("ns".into()),
            all: false,
        };
        let favorite = Favorite::for_view(&info, scope, "Deployments", request);
        assert_eq!(favorite.label(), "Deployments");
        assert!(
            favorite
                .view
                .as_ref()
                .and_then(|v| v.target.as_ref())
                .is_some_and(|t| t.cluster.as_str().is_empty())
        );
        let cluster = ClusterId::new("other@/g");
        let opened = favorite.request(&cluster).unwrap();
        assert_eq!(opened.target.unwrap().cluster, cluster);

        // The same view in another namespace is another favorite; a plain namespace favorite
        // isn't the view's.
        let plain = Favorite::new(&info, "ns");
        assert!(!plain.same_target(&favorite));
        let json = serde_json::to_string(&favorite).unwrap();
        assert_eq!(serde_json::from_str::<Favorite>(&json).unwrap(), favorite);
    }

    #[test]
    fn scopes_follow_the_tab_then_the_target_then_the_title_bar() {
        let one = Scope::of_tab(Some(TabNamespace::One("a".into())), None, Some("t".into()));
        assert_eq!(one.namespace.as_deref(), Some("a"));
        assert!(!one.all);
        let all = Scope::of_tab(Some(TabNamespace::All), Some(None), Some("t".into()));
        assert!(all.namespace.is_none() && all.all);
        // Follows the title bar: the target's namespace, or the title bar's own.
        let target = Scope::of_tab(Some(TabNamespace::Keep), Some(Some("x".into())), None);
        assert_eq!(target.namespace.as_deref(), Some("x"));
        let bar = Scope::of_tab(None, None, Some("t".into()));
        assert_eq!(bar.namespace.as_deref(), Some("t"));
        assert!(!Scope::of_tab(None, None, None).all);
    }

    #[test]
    fn all_namespaces_is_part_of_a_views_identity() {
        let info = context("c", "s", "/f");
        let request = ViewRequest::new(ViewKind::Overview);
        let everywhere = Scope {
            namespace: None,
            all: true,
        };
        let unscoped = Scope::default();
        let a = Favorite::for_view(&info, everywhere, "Overview", request.clone());
        let b = Favorite::for_view(&info, unscoped, "Overview", request);
        assert!(!a.same_target(&b));
        let json = serde_json::to_string(&a).unwrap();
        assert!(
            serde_json::from_str::<Favorite>(&json)
                .unwrap()
                .all_namespaces
        );
    }

    #[test]
    fn favorites_saved_before_views_still_load() {
        let old = r#"{"context":"c","file":"/f","namespace":"payments","kind":"deployments.apps"}"#;
        let favorite: Favorite = serde_json::from_str(old).unwrap();
        assert_eq!(favorite.namespace.as_deref(), Some("payments"));
        assert!(favorite.view.is_none());
        assert!(!favorite.all_namespaces);
        assert_eq!(favorite.label(), "payments");
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
