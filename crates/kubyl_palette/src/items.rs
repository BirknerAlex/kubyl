//! Palette results: built from a [`Snapshot`] of the app state, scored, grouped and sorted.
//! Pure data in and out, so ranking is unit-tested without a window.

use gpui::Hsla;
use kubyl_core::{ClusterId, Gvr, ResourceRef};
use kubyl_resources::StoreKey;
use kubyl_ui::IconName;

use crate::command::{Inline, Mode, Scope, parse_inline};
use crate::matcher::{Match, Query};
use crate::recent::Recent;
use crate::references::{RefTarget, Reference};

/// Kinds listed first when nothing is typed (after recent ones), like k9s's aliases view.
const COMMON_KINDS: &[&str] = &[
    "pods",
    "deployments",
    "services",
    "statefulsets",
    "daemonsets",
    "jobs",
    "cronjobs",
    "ingresses",
    "configmaps",
    "secrets",
    "persistentvolumeclaims",
    "nodes",
    "namespaces",
    "events",
];

/// Results per group next to the mode's own group (`:cert` also shows two actions).
const SECONDARY_LIMIT: usize = 3;
/// Results per group in the no-prefix mode.
const MIXED_LIMIT: usize = 5;
/// Results in the mode's own group.
const PRIMARY_LIMIT: usize = 200;
/// Objects are only searched once the query has this many characters.
const OBJECT_MIN_QUERY: usize = 2;

/// A resource kind served by the active cluster.
#[derive(Clone, Debug)]
pub struct KindEntry {
    pub gvr: Gvr,
    pub kind: String,
    pub singular: String,
    pub short_names: Vec<String>,
    pub categories: Vec<String>,
    pub namespaced: bool,
    pub icon: IconName,
}

#[derive(Clone, Debug)]
pub struct ContextEntry {
    pub id: ClusterId,
    pub name: String,
    pub context: String,
    pub server: Option<String>,
    pub state: String,
    pub color: Hsla,
    pub connected: bool,
    pub connecting: bool,
    pub production: bool,
    /// A group's contexts `(name, namespace)`: their names find the group too.
    pub members: Vec<(String, Option<String>)>,
}

#[derive(Clone, Debug)]
pub struct ActionEntry {
    /// Index into `ActionRegistry::all()`.
    pub index: usize,
    pub name: String,
    /// Keystrokes that run it where the palette was opened.
    pub keys: Option<String>,
    /// How deep its key context matches where the palette was opened (0: app-wide). Actions of
    /// the focused list come first.
    pub depth: u32,
}

#[derive(Clone, Debug)]
pub struct FavoriteEntry {
    /// Index into `Favorites::items()`.
    pub index: usize,
    /// The cluster it resolves to, if its context is loaded.
    pub cluster_id: Option<ClusterId>,
    pub namespace: String,
    pub label: String,
    pub cluster: String,
    /// Plural resource name (`pods`, `deployments.apps`).
    pub kind: String,
    pub selector: Option<String>,
    pub resolved: bool,
}

#[derive(Clone, Debug)]
pub struct ObjectEntry {
    pub target: ResourceRef,
    pub kind: String,
    pub cluster_name: String,
    pub icon: IconName,
}

/// Everything the builder needs, read from the app when the query changes.
#[derive(Clone, Debug, Default)]
pub struct Snapshot {
    /// The active cluster and its display name.
    pub cluster: Option<(ClusterId, String)>,
    /// The active namespace (`None` = all).
    pub namespace: Option<String>,
    pub kinds: Vec<KindEntry>,
    pub contexts: Vec<ContextEntry>,
    pub namespaces: Vec<String>,
    /// Namespaces come from a live listing (otherwise any typed name is offered).
    pub namespaces_listed: bool,
    /// Actions available where the palette was opened.
    pub actions: Vec<ActionEntry>,
    pub favorites: Vec<FavoriteEntry>,
    /// Loaded objects (shared: gathered once per palette).
    pub objects: std::sync::Arc<Vec<ObjectEntry>>,
    /// References of the selection, resolved to kinds of its cluster.
    pub references: Vec<(Reference, Option<ResolvedRef>)>,
    /// Kind label of the focused list (`/` mode), `None` if no list had focus.
    pub list: Option<String>,
    pub recent: Recent,
}

/// A reference resolved against discovery.
#[derive(Clone, Debug, PartialEq)]
pub struct ResolvedRef {
    pub cluster: ClusterId,
    pub gvr: Gvr,
    pub namespaced: bool,
    pub icon: IconName,
}

/// Result groups, in their default display order.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum Group {
    Commands,
    Recent,
    Kinds,
    References,
    Objects,
    Contexts,
    Namespaces,
    Actions,
    Favorites,
    Filter,
}

impl Group {
    pub fn label(self) -> &'static str {
        match self {
            Group::Commands => "Commands",
            Group::Recent => "Recent",
            Group::Kinds => "Resource kinds",
            Group::References => "References",
            Group::Objects => "Objects",
            Group::Contexts => "Contexts",
            Group::Namespaces => "Namespaces",
            Group::Actions => "Actions",
            Group::Favorites => "Favorites",
            Group::Filter => "Filter",
        }
    }

    fn of_mode(mode: Mode) -> Option<Group> {
        Some(match mode {
            Mode::All => return None,
            Mode::Resources => Group::Kinds,
            Mode::Contexts => Group::Contexts,
            Mode::Namespaces => Group::Namespaces,
            Mode::Actions => Group::Actions,
            Mode::Favorites => Group::Favorites,
            Mode::Filter => Group::Filter,
            Mode::Objects => Group::Objects,
            Mode::References => Group::References,
        })
    }
}

/// What confirming a result does.
#[derive(Clone, Debug, PartialEq)]
pub enum Target {
    /// Open the list of a kind.
    Kind {
        cluster: ClusterId,
        gvr: Gvr,
        namespaced: bool,
        scope: Scope,
    },
    Context(ClusterId),
    /// A cluster entry found by one of its contexts' names (`@dev-alex`): opens it in that
    /// context's namespace.
    ContextIn {
        cluster: ClusterId,
        namespace: Option<String>,
    },
    Namespace(Option<String>),
    /// Index into `ActionRegistry::all()`.
    Action(usize),
    /// Index into `Favorites::items()`.
    Favorite(usize),
    AddFavorite {
        cluster: ClusterId,
        namespace: String,
    },
    /// The Favorites workspace for a kind.
    FavoritesWorkspace(Gvr),
    /// Details of one object.
    Object(ResourceRef),
    /// A list with a filter (selector → pods).
    Filtered {
        cluster: ClusterId,
        gvr: Gvr,
        namespace: Option<String>,
        filter: String,
    },
    /// Set the focused list's filter.
    Filter(String),
    /// Switch the palette to another mode.
    Mode(Mode),
    Quit,
    /// Nothing to run; shows a message (e.g. a feature of a later phase).
    Notice(String),
}

/// The right-hand side of a row.
#[derive(Clone, Debug, PartialEq)]
pub enum Trailing {
    None,
    /// Live object count of this store.
    Count(StoreKey),
    /// Key binding (GPUI keystroke syntax).
    Keys(String),
    Text(String),
}

#[derive(Clone, Debug)]
pub struct Item {
    pub group: Group,
    pub icon: IconName,
    pub title: String,
    /// Matched char positions in `title`.
    pub positions: Vec<usize>,
    /// Dim mono text after the title (`cert-manager.io/v1`).
    pub detail: Option<String>,
    /// Dim mono text on the right (`cert, certs`).
    pub aliases: Option<String>,
    pub trailing: Trailing,
    pub score: u32,
    /// Recency key.
    pub key: Option<String>,
    pub target: Target,
    /// Color dot (contexts).
    pub color: Option<Hsla>,
    /// The connection state of a context, like the sidebar's status slot.
    pub status: Option<ContextStatus>,
    /// The active context/namespace.
    pub current: bool,
}

/// The status slot of a context row: a green dot when connected, a pulsing one while connecting.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ContextStatus {
    Connected,
    Connecting,
}

impl Item {
    fn new(group: Group, icon: IconName, title: impl Into<String>, target: Target) -> Self {
        Self {
            group,
            icon,
            title: title.into(),
            positions: Vec::new(),
            detail: None,
            aliases: None,
            trailing: Trailing::None,
            score: 0,
            key: None,
            target,
            color: None,
            status: None,
            current: false,
        }
    }

    fn matched(mut self, m: Match) -> Self {
        self.score = m.score;
        self.positions = m.positions;
        self
    }

    fn detail(mut self, detail: impl Into<String>) -> Self {
        self.detail = Some(detail.into());
        self
    }

    fn key(mut self, key: String) -> Self {
        self.key = Some(key);
        self
    }
}

/// Options that change results without changing the query.
#[derive(Clone, Copy, Debug, Default)]
pub struct Options {
    /// `⇥`: open kinds in all namespaces.
    pub all_namespaces: bool,
}

/// Builds the sorted, grouped results for `query` in `mode`.
pub fn build(mode: Mode, query: &str, snapshot: &Snapshot, options: Options) -> Vec<Item> {
    let mut builder = Builder {
        snapshot,
        options,
        items: Vec::new(),
    };
    let mut q = Query::new(query);
    match mode {
        Mode::All => {
            if q.is_empty() {
                builder.recent();
            }
            builder.kinds(&mut q, Scope::Active);
            builder.actions(&mut q);
            builder.contexts(&mut q);
            builder.namespaces(&mut q, false);
            builder.favorites(&mut q);
            if q.text().chars().count() >= OBJECT_MIN_QUERY {
                builder.objects(&mut q);
            }
        }
        Mode::Resources => builder.resources(query),
        Mode::Contexts => builder.contexts(&mut q),
        Mode::Namespaces => builder.namespaces(&mut q, true),
        Mode::Actions => builder.actions(&mut q),
        Mode::Favorites => {
            builder.favorites(&mut q);
            builder.favorite_workspaces(&mut q);
        }
        Mode::Filter => builder.filter(query),
        Mode::Objects => {
            if !q.is_empty() {
                builder.objects(&mut q);
            }
        }
        Mode::References => builder.references(&mut q),
    }
    arrange(builder.items, mode, &snapshot.recent)
}

struct Builder<'a> {
    snapshot: &'a Snapshot,
    options: Options,
    items: Vec<Item>,
}

impl Builder<'_> {
    fn push(&mut self, item: Item) {
        self.items.push(item);
    }

    fn kind_key(gvr: &Gvr) -> String {
        format!("kind:{}/{}", gvr.group, gvr.resource)
    }

    fn scope(&self, scope: Scope) -> Scope {
        match scope {
            Scope::Active if self.options.all_namespaces => Scope::All,
            other => other,
        }
    }

    /// Store key for the live count of a kind (the namespace the list would show).
    fn count_key(&self, cluster: &ClusterId, kind: &KindEntry, scope: &Scope) -> StoreKey {
        let namespace = match (kind.namespaced, scope) {
            (false, _) | (true, Scope::All) => None,
            (true, Scope::Named(ns)) => Some(ns.clone()),
            (true, Scope::Active) => self.snapshot.namespace.clone(),
        };
        StoreKey::new(cluster.clone(), kind.gvr.clone(), namespace).metadata()
    }

    fn kind_item(&self, cluster: &ClusterId, kind: &KindEntry, m: Match, scope: Scope) -> Item {
        let mut aliases = kind.short_names.clone();
        if aliases.is_empty() && kind.singular != kind.gvr.resource {
            aliases.push(kind.singular.clone());
        }
        let mut detail = kind.gvr.api_version();
        if !kind.namespaced {
            detail.push_str(" · cluster");
        }
        let scope = self.scope(scope);
        let mut item = Item::new(
            Group::Kinds,
            kind.icon,
            kind.gvr.resource.clone(),
            Target::Kind {
                cluster: cluster.clone(),
                gvr: kind.gvr.clone(),
                namespaced: kind.namespaced,
                scope: scope.clone(),
            },
        )
        .matched(m)
        .detail(detail)
        .key(Self::kind_key(&kind.gvr));
        item.aliases = (!aliases.is_empty()).then(|| aliases.join(", "));
        item.trailing = Trailing::Count(self.count_key(cluster, kind, &scope));
        item
    }

    fn kinds(&mut self, q: &mut Query, scope: Scope) {
        let Some((cluster, _)) = self.snapshot.cluster.clone() else {
            return;
        };
        let category = q.text().to_string();
        for kind in &self.snapshot.kinds {
            let qualified = format!("{}.{}", kind.gvr.resource, kind.gvr.group);
            let kind_lower = kind.kind.to_lowercase();
            let mut keys: Vec<&str> = vec![&kind.singular, &kind_lower];
            keys.extend(kind.short_names.iter().map(String::as_str));
            let mut m = q.score(&kind.gvr.resource, &keys);
            // `plural.group` only counts when typed in full (fuzzy on groups matches too much).
            if !kind.gvr.group.is_empty() && q.text() == qualified {
                m = Some(Match {
                    score: crate::matcher::EXACT_BONUS,
                    positions: Vec::new(),
                });
            }
            // `:all` lists the kinds in the `all` category, like kubectl.
            if m.is_none() && !category.is_empty() && kind.categories.contains(&category) {
                m = Some(Match {
                    score: crate::matcher::PREFIX_BONUS,
                    positions: Vec::new(),
                });
            }
            if let Some(mut m) = m {
                if q.is_empty() {
                    m.score = COMMON_KINDS
                        .iter()
                        .position(|k| *k == kind.gvr.resource)
                        .map_or(0, |ix| (COMMON_KINDS.len() - ix) as u32);
                }
                // Core and well-known groups win ties over CRDs, like kubectl.
                if !kind.gvr.group.contains('.') {
                    m.score += 1;
                }
                let item = self.kind_item(&cluster, kind, m, scope.clone());
                self.push(item);
            }
        }
    }

    /// Finds the kind a k9s command names (`po`, `deploy`, `pods`, `deployments.apps`).
    fn resolve_kind(&self, name: &str) -> Option<&KindEntry> {
        let name = name.to_lowercase();
        let (name, group) = match name.split_once('.') {
            Some((n, g)) => (n.to_string(), Some(g.to_string())),
            None => (name, None),
        };
        self.snapshot
            .kinds
            .iter()
            .filter(|k| {
                group.as_ref().is_none_or(|g| &k.gvr.group == g)
                    && (k.gvr.resource == name
                        || k.singular == name
                        || k.short_names.contains(&name)
                        || k.kind.to_lowercase() == name)
            })
            .min_by_key(|k| (!k.gvr.group.is_empty(), k.gvr.group.contains('.')))
    }

    fn resources(&mut self, query: &str) {
        match parse_inline(query) {
            None => {
                let mut q = Query::new(query);
                self.kinds(&mut q, Scope::Active);
                if !q.is_empty() {
                    // Next to kinds, only close matches of actions and favorites.
                    q.set_strict(true);
                    self.actions(&mut q);
                    self.favorites(&mut q);
                }
            }
            Some(Inline::Kind { kind, scope }) => {
                let resolved = self.resolve_kind(&kind).cloned();
                match (resolved, self.snapshot.cluster.clone()) {
                    (Some(entry), Some((cluster, _))) => {
                        let mut item = self.kind_item(
                            &cluster,
                            &entry,
                            Match {
                                score: u32::MAX / 2,
                                positions: Vec::new(),
                            },
                            scope.clone(),
                        );
                        let where_ = match (&scope, entry.namespaced) {
                            (_, false) => "cluster-wide".to_string(),
                            (Scope::All, true) => "in all namespaces".to_string(),
                            (Scope::Named(ns), true) => format!("in {ns}"),
                            (Scope::Active, true) => "here".to_string(),
                        };
                        item.group = Group::Commands;
                        item.title = format!("Open {} {where_}", entry.gvr.resource);
                        self.push(item);
                    }
                    _ => {
                        let mut q = Query::new(&kind);
                        self.kinds(&mut q, scope);
                    }
                }
            }
            Some(Inline::Context(name)) => {
                if name.is_none() {
                    self.command(
                        "List contexts",
                        IconName::ShipWheel,
                        Target::Mode(Mode::Contexts),
                    );
                }
                let mut q = Query::new(name.as_deref().unwrap_or_default());
                self.contexts(&mut q);
            }
            Some(Inline::Namespace(name)) => {
                if name.is_none() {
                    self.command(
                        "List namespaces",
                        IconName::Folder,
                        Target::Mode(Mode::Namespaces),
                    );
                }
                let mut q = Query::new(name.as_deref().unwrap_or_default());
                self.namespaces(&mut q, true);
            }
            Some(Inline::Xray(kind)) => self.command(
                format!("XRay {kind}").trim().to_string(),
                IconName::Network,
                Target::Notice("XRay views arrive with a later phase.".into()),
            ),
            Some(Inline::Quit) => self.command("Quit Kubyl", IconName::X, Target::Quit),
        }
    }

    fn command(&mut self, title: impl Into<String>, icon: IconName, target: Target) {
        let mut item = Item::new(Group::Commands, icon, title, target);
        item.score = u32::MAX / 2;
        self.push(item);
    }

    fn contexts(&mut self, q: &mut Query) {
        let active = self.snapshot.cluster.as_ref().map(|(id, _)| id);
        for ctx in &self.snapshot.contexts {
            let server = ctx.server.clone().unwrap_or_default();
            let direct = q.score(&ctx.name, &[&ctx.context, &server]);
            // A group's member context names are aliases: `@dev-alex` finds the group and
            // opens it in dev-alex (only when the query isn't a match of the label itself).
            let alias = if q.is_empty() || ctx.members.len() < 2 {
                None
            } else {
                ctx.members
                    .iter()
                    .filter_map(|(name, namespace)| {
                        q.score(name, &[])
                            .map(|m| (m, name.clone(), namespace.clone()))
                    })
                    .max_by_key(|(m, _, _)| m.score)
            };
            let (mut m, via) = match (direct, alias) {
                (Some(direct), Some(alias)) if alias.0.score > direct.score => (
                    Match {
                        positions: Vec::new(),
                        ..alias.0
                    },
                    Some((alias.1, alias.2)),
                ),
                (Some(direct), _) => (direct, None),
                (None, Some(alias)) => (
                    Match {
                        positions: Vec::new(),
                        ..alias.0
                    },
                    Some((alias.1, alias.2)),
                ),
                (None, None) => continue,
            };
            let current = active == Some(&ctx.id);
            // Nothing typed: the active context, then connected ones.
            if q.is_empty() {
                m.score += u32::from(current) * 2 + u32::from(ctx.connected);
            }
            let target = match &via {
                Some((_, namespace)) => Target::ContextIn {
                    cluster: ctx.id.clone(),
                    namespace: namespace.clone(),
                },
                None => Target::Context(ctx.id.clone()),
            };
            let mut item = Item::new(
                Group::Contexts,
                IconName::ShipWheel,
                ctx.name.clone(),
                target,
            )
            .matched(m)
            .key(format!("ctx:{}", ctx.id));
            item.detail = match &via {
                Some((name, _)) => Some(format!("context {name}")),
                None => ctx.server.as_deref().map(host_of),
            };
            let state = match &via {
                Some((_, Some(namespace))) => format!("opens in {namespace}"),
                _ => ctx.state.clone(),
            };
            item.trailing = Trailing::Text(if ctx.production {
                format!("PROD · {state}")
            } else {
                state
            });
            item.color = Some(ctx.color);
            item.status = if ctx.connected {
                Some(ContextStatus::Connected)
            } else if ctx.connecting {
                Some(ContextStatus::Connecting)
            } else {
                None
            };
            item.current = current;
            self.push(item);
        }
    }

    fn namespaces(&mut self, q: &mut Query, offer_typed: bool) {
        let Some((_, cluster_name)) = self.snapshot.cluster.clone() else {
            return;
        };
        let current = &self.snapshot.namespace;
        if let Some(m) = q.score("all namespaces", &["-a", "all"]) {
            let mut item = Item::new(
                Group::Namespaces,
                IconName::Layers,
                "all namespaces",
                Target::Namespace(None),
            )
            .matched(m)
            .detail(cluster_name.clone())
            .key("ns:*".into());
            item.current = current.is_none();
            self.push(item);
        }
        for ns in &self.snapshot.namespaces {
            let Some(m) = q.score(ns, &[]) else {
                continue;
            };
            let mut item = Item::new(
                Group::Namespaces,
                IconName::Folder,
                ns.clone(),
                Target::Namespace(Some(ns.clone())),
            )
            .matched(m)
            .key(format!("ns:{ns}"));
            item.current = current.as_deref() == Some(ns);
            self.push(item);
        }
        let typed = q.text().to_string();
        // A name that isn't listed (listing forbidden) can still be used.
        if offer_typed
            && !typed.is_empty()
            && is_dns_label(&typed)
            && !self.snapshot.namespaces.contains(&typed)
        {
            let detail = if self.snapshot.namespaces_listed {
                "not listed".to_string()
            } else {
                "listing forbidden, use the typed name".to_string()
            };
            let mut item = Item::new(
                Group::Namespaces,
                IconName::Folder,
                typed.clone(),
                Target::Namespace(Some(typed)),
            )
            .detail(detail);
            item.score = 1;
            self.push(item);
        }
    }

    fn actions(&mut self, q: &mut Query) {
        for action in &self.snapshot.actions {
            let Some(mut m) = q.score(&action.name, &[]) else {
                continue;
            };
            m.score += action.depth;
            let mut item = Item::new(
                Group::Actions,
                action_icon(&action.name),
                action.name.clone(),
                Target::Action(action.index),
            )
            .matched(m)
            .key(format!("action:{}", action.name));
            if let Some(keys) = &action.keys {
                item.trailing = Trailing::Keys(keys.clone());
            }
            self.push(item);
        }
    }

    fn favorites(&mut self, q: &mut Query) {
        for fav in &self.snapshot.favorites {
            let kind = fav.kind.split('.').next().unwrap_or_default().to_string();
            let Some(m) = q.score(&fav.label, &[&fav.cluster, &kind]) else {
                continue;
            };
            let mut detail = format!("{} · {kind}", fav.cluster);
            if let Some(selector) = &fav.selector {
                detail.push_str(&format!(" · {selector}"));
            }
            if !fav.resolved {
                detail.push_str(" · context not found");
            }
            let item = Item::new(
                Group::Favorites,
                IconName::Star,
                fav.label.clone(),
                Target::Favorite(fav.index),
            )
            .matched(m)
            .detail(detail)
            .key(format!("fav:{}:{}", fav.cluster, fav.label));
            self.push(item);
        }
    }

    /// `*` mode extras: add the current namespace, and the Favorites workspace per kind.
    fn favorite_workspaces(&mut self, q: &mut Query) {
        if let (Some((cluster, name)), Some(ns)) = (
            self.snapshot.cluster.clone(),
            self.snapshot.namespace.clone(),
        ) && !self.snapshot.favorites.iter().any(|f| {
            f.cluster_id.as_ref() == Some(&cluster)
                && f.namespace == ns
                && f.selector.is_none()
                && f.kind == "pods"
        }) && let Some(m) = q.score(&format!("Add {ns} to favorites"), &["add", "star"])
        {
            let mut item = Item::new(
                Group::Commands,
                IconName::StarFilled,
                format!("Add {ns} to favorites"),
                Target::AddFavorite {
                    cluster,
                    namespace: ns,
                },
            )
            .matched(m)
            .detail(name);
            item.score += 1;
            self.push(item);
        }
        let pods = self
            .snapshot
            .kinds
            .iter()
            .find(|k| k.gvr.group.is_empty() && k.gvr.resource == "pods")
            .cloned();
        let candidates: Vec<KindEntry> = if q.is_empty() {
            pods.into_iter().collect()
        } else {
            self.snapshot
                .kinds
                .iter()
                .filter(|k| k.namespaced)
                .cloned()
                .collect()
        };
        for kind in candidates {
            let title = format!("Favorites workspace: {}", kind.gvr.resource);
            let keys: Vec<&str> = kind.short_names.iter().map(String::as_str).collect();
            let Some(m) = q.score(&kind.gvr.resource, &keys) else {
                continue;
            };
            let mut item = Item::new(
                Group::Favorites,
                IconName::Blocks,
                title,
                Target::FavoritesWorkspace(kind.gvr.clone()),
            )
            .detail("every favorite in one table")
            .key(format!("favws:{}", kind.gvr.resource));
            item.score = m.score;
            self.push(item);
        }
    }

    fn objects(&mut self, q: &mut Query) {
        for object in self.snapshot.objects.iter() {
            let name = object.target.name.clone().unwrap_or_default();
            let Some(m) = q.score(&name, &[]) else {
                continue;
            };
            let mut detail = object.kind.clone();
            if let Some(ns) = &object.target.namespace {
                detail.push_str(&format!(" · {ns}"));
            }
            detail.push_str(&format!(" · {}", object.cluster_name));
            let item = Item::new(
                Group::Objects,
                object.icon,
                name,
                Target::Object(object.target.clone()),
            )
            .matched(m)
            .detail(detail);
            self.push(item);
        }
    }

    fn references(&mut self, q: &mut Query) {
        for (reference, resolved) in &self.snapshot.references {
            let title = reference.title();
            let Some(m) = q.score(&title, &[reference.relation]) else {
                continue;
            };
            let (target, icon) = match (&reference.target, resolved) {
                (_, None) => (
                    Target::Notice(format!("{title}: this cluster doesn't serve that kind.")),
                    IconName::TriangleAlert,
                ),
                (
                    RefTarget::Object {
                        namespace, name, ..
                    },
                    Some(r),
                ) => (
                    Target::Object(ResourceRef::object(
                        r.cluster.clone(),
                        r.gvr.clone(),
                        namespace.clone().filter(|_| r.namespaced),
                        name.clone(),
                    )),
                    r.icon,
                ),
                (
                    RefTarget::Filtered {
                        namespace, filter, ..
                    },
                    Some(r),
                ) => (
                    Target::Filtered {
                        cluster: r.cluster.clone(),
                        gvr: r.gvr.clone(),
                        namespace: namespace.clone(),
                        filter: filter.clone(),
                    },
                    r.icon,
                ),
            };
            let item = Item::new(Group::References, icon, title, target)
                .matched(m)
                .detail(reference.relation);
            self.push(item);
        }
    }

    fn filter(&mut self, query: &str) {
        let Some(list) = self.snapshot.list.clone() else {
            let item = Item::new(
                Group::Filter,
                IconName::Funnel,
                "Focus a list first",
                Target::Notice("The / mode filters the focused list. Open a list first.".into()),
            );
            self.push(item);
            return;
        };
        let query = query.trim();
        let (title, detail) = if query.is_empty() {
            (format!("Clear the {list} filter"), None)
        } else {
            (
                format!("Filter {list}: {query}"),
                Some("text, !text, /regex/, label=value, status.phase=Running".to_string()),
            )
        };
        let mut item = Item::new(
            Group::Filter,
            IconName::Funnel,
            title,
            Target::Filter(query.to_string()),
        );
        item.detail = detail;
        self.push(item);
    }

    /// Recently used kinds, actions, contexts and namespaces (empty query, no prefix).
    fn recent(&mut self) {
        let mut all = Builder {
            snapshot: self.snapshot,
            options: self.options,
            items: Vec::new(),
        };
        let mut q = Query::new("");
        all.kinds(&mut q, Scope::Active);
        all.actions(&mut q);
        all.contexts(&mut q);
        all.namespaces(&mut q, false);
        all.favorites(&mut q);
        let recent = &self.snapshot.recent;
        let mut picked: Vec<Item> = all
            .items
            .into_iter()
            .filter(|i| i.key.as_ref().is_some_and(|k| recent.rank(k).is_some()))
            .collect();
        picked.sort_by_key(|i| i.key.as_ref().and_then(|k| recent.rank(k)));
        for (rank, mut item) in picked.into_iter().take(8).enumerate() {
            item.group = Group::Recent;
            // Ordered by recency alone (`arrange` adds no boost to this group).
            item.score = u32::MAX / 4 - rank as u32;
            self.push(item);
        }
    }
}

/// Applies the recency boost, sorts within groups, orders groups and caps them.
fn arrange(mut items: Vec<Item>, mode: Mode, recent: &Recent) -> Vec<Item> {
    for item in items.iter_mut().filter(|i| i.group != Group::Recent) {
        if let Some(key) = &item.key {
            item.score += recent.boost(key);
        }
    }
    let primary = Group::of_mode(mode);
    // Groups by their best score, the mode's own group and commands first.
    let mut groups: Vec<(Group, u32)> = Vec::new();
    for item in &items {
        match groups.iter_mut().find(|(g, _)| *g == item.group) {
            Some((_, best)) => *best = (*best).max(item.score),
            None => groups.push((item.group, item.score)),
        }
    }
    groups.sort_by_key(|(group, best)| {
        let rank = match *group {
            Group::Commands => 0,
            Group::Recent => 1,
            g if Some(g) == primary => 2,
            _ => 3,
        };
        // In the mixed mode the best match leads; elsewhere the fixed order.
        let score = if mode == Mode::All && rank == 3 {
            u32::MAX - *best
        } else {
            0
        };
        (rank, score, *group)
    });
    let mut out = Vec::new();
    for (group, _) in groups {
        let mut members: Vec<Item> = items.iter().filter(|i| i.group == group).cloned().collect();
        // Stable: equal scores keep the source order (registry order, sorted names…).
        members.sort_by_key(|i| std::cmp::Reverse(i.score));
        let limit = match group {
            Group::Commands | Group::Recent => usize::MAX,
            g if Some(g) == primary => PRIMARY_LIMIT,
            _ if mode == Mode::All => MIXED_LIMIT,
            _ => SECONDARY_LIMIT,
        };
        out.extend(members.into_iter().take(limit));
    }
    items.clear();
    out
}

/// `https://10.0.0.1:6443` → `10.0.0.1:6443`.
fn host_of(server: &str) -> String {
    server
        .split("://")
        .nth(1)
        .unwrap_or(server)
        .trim_end_matches('/')
        .to_string()
}

fn is_dns_label(name: &str) -> bool {
    !name.is_empty()
        && name.len() <= 63
        && name
            .chars()
            .all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '-')
}

fn action_icon(name: &str) -> IconName {
    let lower = name.to_lowercase();
    if lower.contains("delete") || lower.contains("kill") {
        IconName::Trash
    } else if lower.contains("restart") || lower.contains("undo") {
        IconName::RefreshCw
    } else if lower.contains("scale") {
        IconName::SlidersVertical
    } else if lower.contains("describe") || lower.contains("details") {
        IconName::Eye
    } else if lower.contains("copy") {
        IconName::Copy
    } else if lower.contains("filter") {
        IconName::Funnel
    } else if lower.contains("namespace") {
        IconName::Folder
    } else if lower.contains("favorite") {
        IconName::Star
    } else if lower.contains("split") || lower.contains("dock") || lower.contains("sidebar") {
        IconName::Columns
    } else {
        IconName::Zap
    }
}

#[cfg(test)]
mod tests {
    use kubyl_core::Gvk;

    use super::*;

    fn kind(group: &str, resource: &str, kind: &str, short: &[&str]) -> KindEntry {
        KindEntry {
            gvr: Gvr::new(group, "v1", resource),
            kind: kind.into(),
            singular: kind.to_lowercase(),
            short_names: short.iter().map(|s| s.to_string()).collect(),
            categories: if group.is_empty() || group == "apps" {
                vec!["all".into()]
            } else {
                Vec::new()
            },
            namespaced: resource != "nodes" && resource != "certificatesigningrequests",
            icon: IconName::File,
        }
    }

    fn snapshot() -> Snapshot {
        Snapshot {
            cluster: Some((ClusterId::new("dev@/k"), "kind-dev".into())),
            namespace: Some("payments".into()),
            kinds: vec![
                kind("", "pods", "Pod", &["po"]),
                kind("apps", "deployments", "Deployment", &["deploy"]),
                kind("", "secrets", "Secret", &[]),
                kind("", "nodes", "Node", &["no"]),
                kind(
                    "certificates.k8s.io",
                    "certificatesigningrequests",
                    "CertificateSigningRequest",
                    &["csr"],
                ),
                kind(
                    "cert-manager.io",
                    "certificaterequests",
                    "CertificateRequest",
                    &["cr", "crs"],
                ),
                kind(
                    "cert-manager.io",
                    "certificates",
                    "Certificate",
                    &["cert", "certs"],
                ),
            ],
            contexts: vec![
                ContextEntry {
                    id: ClusterId::new("dev@/k"),
                    name: "kind-dev".into(),
                    context: "kind-dev".into(),
                    server: Some("https://127.0.0.1:6443".into()),
                    state: "connected".into(),
                    color: gpui::red(),
                    connected: true,
                    connecting: false,
                    production: false,
                    members: Vec::new(),
                },
                ContextEntry {
                    id: ClusterId::new("staging@/k"),
                    name: "staging-eu-west-1".into(),
                    context: "staging-eu-west-1".into(),
                    server: None,
                    state: "disconnected".into(),
                    color: gpui::blue(),
                    connected: false,
                    connecting: false,
                    production: true,
                    members: Vec::new(),
                },
            ],
            namespaces: vec!["default".into(), "kube-system".into(), "payments".into()],
            namespaces_listed: true,
            actions: vec![
                ActionEntry {
                    index: 0,
                    name: "Resource: Delete…".into(),
                    keys: Some("ctrl-d".into()),
                    depth: 4,
                },
                ActionEntry {
                    index: 1,
                    name: "Certificate: Renew".into(),
                    keys: None,
                    depth: 0,
                },
            ],
            favorites: vec![FavoriteEntry {
                index: 0,
                cluster_id: Some(ClusterId::new("staging@/k")),
                namespace: "payments".into(),
                label: "payments".into(),
                cluster: "staging-eu-west-1".into(),
                kind: "certificates.cert-manager.io".into(),
                selector: None,
                resolved: true,
            }],
            ..Default::default()
        }
    }

    fn titles(items: &[Item]) -> Vec<(Group, String)> {
        items.iter().map(|i| (i.group, i.title.clone())).collect()
    }

    #[test]
    fn cert_shows_certificates_first_then_actions_and_favorites() {
        let items = build(Mode::Resources, "cert", &snapshot(), Options::default());
        assert_eq!(items[0].title, "certificates");
        assert_eq!(items[0].group, Group::Kinds);
        assert_eq!(items[0].aliases.as_deref(), Some("cert, certs"));
        assert_eq!(items[0].detail.as_deref(), Some("cert-manager.io/v1"));
        assert_eq!(items[0].positions, vec![0, 1, 2, 3]);
        let Trailing::Count(key) = &items[0].trailing else {
            panic!("no count")
        };
        assert_eq!(key.namespace.as_deref(), Some("payments"));
        let groups: Vec<Group> = items.iter().map(|i| i.group).collect();
        let first_action = groups.iter().position(|g| *g == Group::Actions).unwrap();
        let first_favorite = groups.iter().position(|g| *g == Group::Favorites).unwrap();
        assert!(groups[..first_action].iter().all(|g| *g == Group::Kinds));
        assert!(first_action < first_favorite);
        assert!(titles(&items).contains(&(Group::Actions, "Certificate: Renew".into())));
    }

    #[test]
    fn short_names_and_categories() {
        let items = build(Mode::Resources, "po", &snapshot(), Options::default());
        assert_eq!(items[0].title, "pods");
        let items = build(Mode::Resources, "deploy", &snapshot(), Options::default());
        assert_eq!(items[0].title, "deployments");
        let items = build(Mode::Resources, "all", &snapshot(), Options::default());
        let kinds: Vec<_> = items
            .iter()
            .filter(|i| i.group == Group::Kinds)
            .map(|i| i.title.as_str())
            .collect();
        assert!(kinds.contains(&"pods") && kinds.contains(&"deployments"));
        assert!(!kinds.contains(&"certificates"));
    }

    #[test]
    fn inline_commands() {
        let s = snapshot();
        let items = build(Mode::Resources, "pods kube-system", &s, Options::default());
        assert_eq!(items[0].group, Group::Commands);
        assert_eq!(items[0].title, "Open pods in kube-system");
        assert!(matches!(
            &items[0].target,
            Target::Kind { scope: Scope::Named(ns), .. } if ns == "kube-system"
        ));
        let items = build(Mode::Resources, "deploy -A", &s, Options::default());
        assert!(matches!(
            &items[0].target,
            Target::Kind { scope: Scope::All, gvr, .. } if gvr.resource == "deployments"
        ));
        let Trailing::Count(key) = &items[0].trailing else {
            panic!()
        };
        assert_eq!(key.namespace, None);

        let items = build(Mode::Resources, "ctx staging", &s, Options::default());
        assert_eq!(items[0].title, "staging-eu-west-1");
        assert_eq!(
            items[0].target,
            Target::Context(ClusterId::new("staging@/k"))
        );
        let items = build(Mode::Resources, "ns kube", &s, Options::default());
        assert_eq!(
            items[0].target,
            Target::Namespace(Some("kube-system".into()))
        );
        let items = build(Mode::Resources, "q", &s, Options::default());
        assert_eq!(items[0].target, Target::Quit);
        let items = build(Mode::Resources, "xray deploy", &s, Options::default());
        assert!(matches!(items[0].target, Target::Notice(_)));
    }

    #[test]
    fn tab_opens_in_all_namespaces() {
        let items = build(
            Mode::Resources,
            "po",
            &snapshot(),
            Options {
                all_namespaces: true,
            },
        );
        assert!(matches!(
            items[0].target,
            Target::Kind {
                scope: Scope::All,
                ..
            }
        ));
    }

    #[test]
    fn namespaces_offer_all_and_typed_names() {
        let mut s = snapshot();
        let items = build(Mode::Namespaces, "", &s, Options::default());
        assert_eq!(items[0].title, "all namespaces");
        assert!(items.iter().any(|i| i.title == "payments" && i.current));
        s.namespaces_listed = false;
        let items = build(Mode::Namespaces, "team-a", &s, Options::default());
        assert_eq!(
            items.last().unwrap().target,
            Target::Namespace(Some("team-a".into()))
        );
    }

    #[test]
    fn member_context_names_are_aliases_of_their_group() {
        let mut s = snapshot();
        s.contexts.push(ContextEntry {
            id: ClusterId::new("group:c,u@/k/"),
            name: "ocp.eu1.example.com · jane".into(),
            context: "shop/api-ocp-eu1/jane".into(),
            server: Some("https://api.ocp.eu1.example.com:6443".into()),
            state: "Not connected".into(),
            color: gpui::green(),
            connected: false,
            connecting: false,
            production: false,
            members: vec![
                ("shop/api-ocp-eu1/jane".into(), Some("shop".into())),
                ("dev-alex/api-ocp-eu1/jane".into(), Some("dev-alex".into())),
            ],
        });
        let items = build(Mode::Contexts, "dev-alex", &s, Options::default());
        assert_eq!(items[0].title, "ocp.eu1.example.com · jane");
        assert_eq!(
            items[0].target,
            Target::ContextIn {
                cluster: ClusterId::new("group:c,u@/k/"),
                namespace: Some("dev-alex".into())
            }
        );
        assert_eq!(
            items[0].trailing,
            Trailing::Text("opens in dev-alex".into())
        );
        // The label itself still opens the group as usual.
        let items = build(Mode::Contexts, "ocp.eu1", &s, Options::default());
        assert_eq!(
            items[0].target,
            Target::Context(ClusterId::new("group:c,u@/k/"))
        );
    }

    #[test]
    fn contexts_show_their_connection_state() {
        let mut s = snapshot();
        s.contexts[1].connecting = true;
        let items = build(Mode::Contexts, "", &s, Options::default());
        let status = |title: &str| items.iter().find(|i| i.title == title).unwrap().status;
        assert_eq!(status("kind-dev"), Some(ContextStatus::Connected));
        assert_eq!(status("staging-eu-west-1"), Some(ContextStatus::Connecting));
        s.contexts[1].connecting = false;
        let items = build(Mode::Contexts, "", &s, Options::default());
        let staging = items
            .iter()
            .find(|i| i.title == "staging-eu-west-1")
            .unwrap();
        assert_eq!(staging.status, None);
    }

    #[test]
    fn recency_boosts_and_fills_the_empty_palette() {
        let mut s = snapshot();
        s.recent.keys = vec!["kind:/secrets".into(), "ctx:staging@/k".into()];
        let items = build(Mode::Resources, "", &s, Options::default());
        assert_eq!(items[0].title, "secrets");
        let items = build(Mode::All, "", &s, Options::default());
        assert_eq!(items[0].group, Group::Recent);
        assert_eq!(items[0].title, "secrets");
        assert_eq!(items[1].title, "staging-eu-west-1");
        let items = build(Mode::Contexts, "", &s, Options::default());
        assert_eq!(items[0].title, "staging-eu-west-1");
    }

    #[test]
    fn actions_of_the_focused_list_come_first() {
        let mut s = snapshot();
        s.actions.reverse();
        let items = build(Mode::Actions, "", &s, Options::default());
        assert_eq!(items[0].title, "Resource: Delete…");
        assert_eq!(items[0].trailing, Trailing::Keys("ctrl-d".into()));
    }

    #[test]
    fn filter_mode_needs_a_list() {
        let mut s = snapshot();
        let items = build(Mode::Filter, "web", &s, Options::default());
        assert!(matches!(items[0].target, Target::Notice(_)));
        s.list = Some("Pods".into());
        let items = build(Mode::Filter, "app=web", &s, Options::default());
        assert_eq!(items[0].target, Target::Filter("app=web".into()));
    }

    #[test]
    fn objects_and_references() {
        let mut s = snapshot();
        let pod = ResourceRef::object(
            ClusterId::new("dev@/k"),
            Gvr::new("", "v1", "pods"),
            Some("web".into()),
            "checkout-api-7d9".into(),
        );
        s.objects = std::sync::Arc::new(vec![ObjectEntry {
            target: pod.clone(),
            kind: "Pod".into(),
            cluster_name: "kind-dev".into(),
            icon: IconName::Box,
        }]);
        let items = build(Mode::Objects, "chk", &s, Options::default());
        assert_eq!(items[0].target, Target::Object(pod));
        assert_eq!(items[0].detail.as_deref(), Some("Pod · web · kind-dev"));
        // One character doesn't search objects in the mixed mode.
        assert!(
            build(Mode::All, "c", &s, Options::default())
                .iter()
                .all(|i| i.group != Group::Objects)
        );

        s.references = vec![
            (
                Reference {
                    relation: "Node",
                    target: RefTarget::Object {
                        gvk: Gvk::new("", "v1", "Node"),
                        namespace: Some("web".into()),
                        name: "n1".into(),
                    },
                },
                Some(ResolvedRef {
                    cluster: ClusterId::new("dev@/k"),
                    gvr: Gvr::new("", "v1", "nodes"),
                    namespaced: false,
                    icon: IconName::Server,
                }),
            ),
            (
                Reference {
                    relation: "Owner",
                    target: RefTarget::Object {
                        gvk: Gvk::new("example.com", "v1", "Widget"),
                        namespace: None,
                        name: "w".into(),
                    },
                },
                None,
            ),
        ];
        let items = build(Mode::References, "", &s, Options::default());
        assert_eq!(
            items[0].target,
            Target::Object(ResourceRef::object(
                ClusterId::new("dev@/k"),
                Gvr::new("", "v1", "nodes"),
                None,
                "n1".into()
            ))
        );
        assert!(matches!(items[1].target, Target::Notice(_)));
    }

    #[test]
    fn favorites_mode_offers_add_and_workspaces() {
        let s = snapshot();
        let items = build(Mode::Favorites, "", &s, Options::default());
        assert!(matches!(items[0].target, Target::AddFavorite { .. }));
        assert!(items.iter().any(|i| i.target == Target::Favorite(0)));
        assert!(
            items.iter().any(
                |i| matches!(&i.target, Target::FavoritesWorkspace(g) if g.resource == "pods")
            )
        );
        // Already a favorite: not offered again.
        let mut s2 = s.clone();
        s2.favorites[0].cluster_id = Some(ClusterId::new("dev@/k"));
        s2.favorites[0].kind = "pods".into();
        let items = build(Mode::Favorites, "", &s2, Options::default());
        assert!(!matches!(items[0].target, Target::AddFavorite { .. }));
        let items = build(Mode::Favorites, "deploy", &s, Options::default());
        assert!(items.iter().any(
            |i| matches!(&i.target, Target::FavoritesWorkspace(g) if g.resource == "deployments")
        ));
    }
}
