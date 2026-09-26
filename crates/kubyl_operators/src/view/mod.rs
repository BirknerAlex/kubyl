//! The Operators tab (board 7): one per cluster, with the sub-tabs Installed · Install plans ·
//! Subscriptions · Helm releases (· Extensions with OLM v1), each a list with a details pane.

mod extensions;
mod helm;
mod installed;
mod plans;
mod states;
mod subscriptions;

use std::collections::HashMap;
use std::time::Duration;

use gpui::{
    AnyElement, App, AppContext as _, Context, Entity, FocusHandle, Focusable, FontWeight, Global,
    IntoElement, KeyBinding, Render, ScrollStrategy, SharedString, Subscription, Task,
    UniformListScrollHandle, Window, actions, div, prelude::*,
};
use gpui_component::input::{Input, InputEvent, InputState};
use kubyl_core::actions::OpenView;
use kubyl_core::{
    ActionRegistry, ActionSpec, ActiveContext, ClusterId, Gvr, ResourceRef, TabView, ViewKind,
    ViewRegistry, ViewRequest,
};
use kubyl_kube::ConnectionManager;
use kubyl_ui::{ActiveColors, Colors, Icon, IconName, fonts, h_flex, sizes, u, v_flex};

use crate::helm::service::{Helm, HelmLease};
use crate::service::{Availability, Olm, OlmLease, Snapshot};
use crate::widgets;

pub(crate) use helm::HelmResources;
pub(crate) use helm::{context_name as helm_context, status_tone as helm_status_tone};
pub(crate) use installed::Instances;

/// `ViewKind::Custom` of the Helm Releases sidebar row: it opens the Operators tab on its Helm
/// releases sub-tab.
pub const HELM_KIND: &str = "helm_releases";

/// Key contexts: the view, and the list of each sub-tab (keys work there, never in the filter).
pub const VIEW_CONTEXT: &str = "OperatorsView";
pub const INSTALLED_CONTEXT: &str = "OperatorsInstalled";
pub const PLANS_CONTEXT: &str = "OperatorsPlans";
pub const SUBSCRIPTIONS_CONTEXT: &str = "OperatorsSubscriptions";
pub const HELM_CONTEXT: &str = "HelmReleases";
pub const EXTENSIONS_CONTEXT: &str = "OperatorsExtensions";

actions!(
    operators,
    [
        /// Shows or hides the details of the selected row.
        Details,
        /// Approves the selected operator's (or plan's) pending install plan.
        Approve,
        /// Reviews the pending install plan: CRD changes, RBAC, compatibility.
        ReviewChanges,
        /// Creates an instance of one of the operator's APIs from its examples.
        CreateInstance,
        /// Opens the selected object's YAML.
        ViewYaml,
        /// Edits the selected object's YAML.
        EditYaml,
        /// Uninstalls the selected operator.
        Uninstall,
        /// Opens the selected Helm release.
        OpenRelease,
        /// Opens the selected release on its values.
        ReleaseValues,
        /// Opens the selected release on its manifest.
        ReleaseManifest,
        /// Opens the selected release on its history.
        ReleaseHistory,
        /// Copies a `helm` command for the selected release.
        HelmCommands,
        /// Opens a ClusterExtension template.
        NewExtension,
        /// Focuses the filter.
        FocusFilter,
        /// Back from the filter to the list.
        BlurFilter,
        /// Next row.
        SelectNext,
        /// Previous row.
        SelectPrevious,
        /// Shows the active cluster's installed operators.
        ShowOperators,
        /// Shows the active cluster's Helm releases.
        ShowHelmReleases,
        /// Shows the active cluster's install plans.
        ShowInstallPlans,
    ]
);

/// Hints of writing actions, hidden on read-only clusters.
pub const WRITE_HINTS: &[&str] = &[
    "Approve…",
    "Create instance…",
    "Uninstall…",
    "Edit YAML",
    "New ClusterExtension…",
];

/// The sub-tabs.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash)]
pub enum SubTab {
    #[default]
    Installed,
    Plans,
    Subscriptions,
    Helm,
    Extensions,
}

impl SubTab {
    fn context(self) -> &'static str {
        match self {
            SubTab::Installed => INSTALLED_CONTEXT,
            SubTab::Plans => PLANS_CONTEXT,
            SubTab::Subscriptions => SUBSCRIPTIONS_CONTEXT,
            SubTab::Helm => HELM_CONTEXT,
            SubTab::Extensions => EXTENSIONS_CONTEXT,
        }
    }

    fn placeholder(self) -> &'static str {
        match self {
            SubTab::Installed => "Filter operators",
            SubTab::Plans => "Filter install plans",
            SubTab::Subscriptions => "Filter subscriptions",
            SubTab::Helm => "Filter releases, charts, namespaces",
            SubTab::Extensions => "Filter extensions",
        }
    }

    fn needs_olm(self) -> bool {
        !matches!(self, SubTab::Helm)
    }
}

/// What the next Operators tab of a cluster opens with.
#[derive(Clone, Debug, Default)]
pub struct Pending {
    pub tab: Option<SubTab>,
    /// A row key to select (`sub:ns/name`, `plan:ns/name`, `helm:ns/name`…).
    pub select: Option<String>,
}

#[derive(Default)]
struct PendingOpens(HashMap<ClusterId, Pending>);

impl Global for PendingOpens {}

pub(crate) fn request(cluster: &ClusterId) -> ViewRequest {
    ViewRequest::for_resource(
        ViewKind::Operators,
        ResourceRef::list(cluster.clone(), Gvr::new("", "", ""), None),
    )
}

/// Opens (or focuses) the Operators tab of `cluster` with a pending sub-tab or selection.
pub fn open(cluster: &ClusterId, pending: Pending, window: &mut Window, cx: &mut App) {
    cx.default_global::<PendingOpens>()
        .0
        .insert(cluster.clone(), pending);
    window.dispatch_action(Box::new(OpenView(request(cluster))), cx);
    cx.refresh_windows();
}

pub(crate) fn init(cx: &mut App) {
    ViewRegistry::register(cx, ViewKind::Operators, |request, window, cx| {
        let cluster = request.target.as_ref()?.cluster.clone();
        Some(Box::new(
            cx.new(|cx| OperatorsView::new(cluster, window, cx)),
        ))
    });
    // The Helm Releases row: the Operators tab on Helm releases (an open one switches).
    ViewRegistry::register(
        cx,
        ViewKind::Custom(HELM_KIND.into()),
        |request, window, cx| {
            let target = request.target.as_ref()?;
            let cluster = target.cluster.clone();
            cx.default_global::<PendingOpens>().0.insert(
                cluster.clone(),
                Pending {
                    tab: Some(SubTab::Helm),
                    select: None,
                },
            );
            let namespace = target.namespace.clone();
            Some(Box::new(cx.new(|cx| {
                let mut view = OperatorsView::new(cluster, window, cx);
                view.helm_namespace = namespace;
                view
            })))
        },
    );
    actions_init(cx);
}

fn actions_init(cx: &mut App) {
    let keyed: Vec<(ActionSpec, &str, &str)> = vec![
        (
            ActionSpec::new("Operators: Details", Details).hint("Details"),
            "enter",
            INSTALLED_CONTEXT,
        ),
        (
            ActionSpec::new("Operators: Approve Upgrade…", Approve).hint("Approve…"),
            "a",
            INSTALLED_CONTEXT,
        ),
        (
            ActionSpec::new("Operators: Review Changes", ReviewChanges).hint("Review changes"),
            "d",
            INSTALLED_CONTEXT,
        ),
        (
            ActionSpec::new("Operators: Create Instance…", CreateInstance).hint("Create instance…"),
            "c",
            INSTALLED_CONTEXT,
        ),
        (
            ActionSpec::new("Operators: View CSV YAML", ViewYaml).hint("View YAML"),
            "y",
            INSTALLED_CONTEXT,
        ),
        (
            ActionSpec::new("Operators: Uninstall…", Uninstall).hint("Uninstall…"),
            "ctrl-d",
            INSTALLED_CONTEXT,
        ),
        (
            ActionSpec::new("Install Plans: Details", Details).hint("Details"),
            "enter",
            PLANS_CONTEXT,
        ),
        (
            ActionSpec::new("Install Plans: Approve…", Approve).hint("Approve…"),
            "a",
            PLANS_CONTEXT,
        ),
        (
            ActionSpec::new("Install Plans: Review Changes", ReviewChanges).hint("Review changes"),
            "d",
            PLANS_CONTEXT,
        ),
        (
            ActionSpec::new("Install Plans: View YAML", ViewYaml).hint("View YAML"),
            "y",
            PLANS_CONTEXT,
        ),
        (
            ActionSpec::new("Subscriptions: Details", Details).hint("Details"),
            "enter",
            SUBSCRIPTIONS_CONTEXT,
        ),
        (
            ActionSpec::new("Subscriptions: View YAML", ViewYaml).hint("View YAML"),
            "y",
            SUBSCRIPTIONS_CONTEXT,
        ),
        (
            ActionSpec::new("Subscriptions: Edit YAML", EditYaml).hint("Edit YAML"),
            "e",
            SUBSCRIPTIONS_CONTEXT,
        ),
        (
            ActionSpec::new("Subscriptions: Uninstall…", Uninstall).hint("Uninstall…"),
            "ctrl-d",
            SUBSCRIPTIONS_CONTEXT,
        ),
        (
            ActionSpec::new("Helm: Open Release", OpenRelease).hint("Open release"),
            "enter",
            HELM_CONTEXT,
        ),
        (
            ActionSpec::new("Helm: Values", ReleaseValues).hint("Values"),
            "v",
            HELM_CONTEXT,
        ),
        (
            ActionSpec::new("Helm: Manifest", ReleaseManifest).hint("Manifest"),
            "m",
            HELM_CONTEXT,
        ),
        (
            ActionSpec::new("Helm: History", ReleaseHistory).hint("History"),
            "h",
            HELM_CONTEXT,
        ),
        (
            ActionSpec::new("Helm: Copy helm Command…", HelmCommands).hint("Copy helm command"),
            "c",
            HELM_CONTEXT,
        ),
        (
            ActionSpec::new("Extensions: Details", Details).hint("Details"),
            "enter",
            EXTENSIONS_CONTEXT,
        ),
        (
            ActionSpec::new("Extensions: Edit YAML", EditYaml).hint("Edit YAML"),
            "e",
            EXTENSIONS_CONTEXT,
        ),
        (
            ActionSpec::new("Extensions: New ClusterExtension…", NewExtension)
                .hint("New ClusterExtension…"),
            "n",
            EXTENSIONS_CONTEXT,
        ),
    ];
    for (spec, keys, context) in keyed {
        ActionRegistry::register(cx, spec.bind(keys, Some(context)));
    }
    let nav = [
        INSTALLED_CONTEXT,
        PLANS_CONTEXT,
        SUBSCRIPTIONS_CONTEXT,
        HELM_CONTEXT,
        EXTENSIONS_CONTEXT,
    ]
    .join(" || ");
    cx.bind_keys([
        KeyBinding::new("j", SelectNext, Some(nav.as_str())),
        KeyBinding::new("down", SelectNext, Some(nav.as_str())),
        KeyBinding::new("k", SelectPrevious, Some(nav.as_str())),
        KeyBinding::new("up", SelectPrevious, Some(nav.as_str())),
        KeyBinding::new("/", FocusFilter, Some(nav.as_str())),
        KeyBinding::new("escape", BlurFilter, Some("OperatorsView > Input")),
        KeyBinding::new("down", BlurFilter, Some("OperatorsView > Input")),
    ]);
    for spec in [
        ActionSpec::new("Operators: Show Installed Operators", ShowOperators),
        ActionSpec::new("Operators: Show Install Plans", ShowInstallPlans),
        ActionSpec::new("Helm: Show Releases", ShowHelmReleases),
    ] {
        ActionRegistry::register(cx, spec);
    }
    let open_on = |tab: SubTab| {
        move |cx: &mut App| {
            let Some(cluster) = ActiveContext::global(cx)
                .cluster
                .as_ref()
                .map(|c| c.id.clone())
            else {
                return;
            };
            with_window(cx, move |window, cx| {
                open(
                    &cluster,
                    Pending {
                        tab: Some(tab),
                        select: None,
                    },
                    window,
                    cx,
                )
            });
        }
    };
    let installed = open_on(SubTab::Installed);
    cx.on_action(move |_: &ShowOperators, cx| installed(cx));
    let plans = open_on(SubTab::Plans);
    cx.on_action(move |_: &ShowInstallPlans, cx| plans(cx));
    let helm = open_on(SubTab::Helm);
    cx.on_action(move |_: &ShowHelmReleases, cx| helm(cx));
}

/// Runs `f` in the focused window (deferred: global handlers run inside the window update).
pub(crate) fn with_window(cx: &mut App, f: impl FnOnce(&mut Window, &mut App) + 'static) {
    cx.defer(move |cx| {
        let window = cx.active_window().or_else(|| cx.windows().first().copied());
        if let Some(window) = window {
            window.update(cx, |_, window, cx| f(window, cx)).ok();
        }
    });
}

pub struct OperatorsView {
    pub(crate) cluster: ClusterId,
    pub(crate) tab: SubTab,
    pub(crate) filter: Entity<InputState>,
    pub(crate) focus: FocusHandle,
    pub(crate) scroll: UniformListScrollHandle,
    /// The selected row of each sub-tab (by key).
    pub(crate) selected: HashMap<SubTab, String>,
    pub(crate) details_open: bool,
    /// The keys of the selectable rows of the current sub-tab, in order (keyboard navigation).
    pub(crate) keys: Vec<String>,
    /// Namespace the Helm list is limited to (opened from a favorite).
    pub(crate) helm_namespace: Option<String>,
    pub(crate) instances: Option<Instances>,
    pub(crate) helm_resources: Option<(String, HelmResources)>,
    // The rows each list shows (built on render, read by its `uniform_list`).
    pub(crate) installed_rows: Vec<crate::olm::join::Operator>,
    pub(crate) plan_rows: Vec<plans::PlanRow>,
    pub(crate) subscription_rows: Vec<std::sync::Arc<crate::olm::model::Subscription>>,
    pub(crate) helm_rows: Vec<crate::helm::service::ReleaseRow>,
    _olm: Option<OlmLease>,
    _helm: Option<HelmLease>,
    _ticker: Task<()>,
    _subscriptions: Vec<Subscription>,
}

impl OperatorsView {
    pub fn new(cluster: ClusterId, window: &mut Window, cx: &mut Context<Self>) -> Self {
        let filter = cx.new(|cx| InputState::new(window, cx).placeholder("Filter operators"));
        let mut subscriptions = vec![cx.subscribe_in(
            &filter,
            window,
            |this, _, event: &InputEvent, window, cx| match event {
                InputEvent::Change => {
                    this.scroll.scroll_to_item(0, ScrollStrategy::Top);
                    cx.notify();
                }
                InputEvent::PressEnter { .. } => this.focus.focus(window, cx),
                _ => {}
            },
        )];
        if let Some(olm) = Olm::global(cx) {
            subscriptions.push(cx.observe(&olm, |_, _, cx| cx.notify()));
        }
        if let Some(helm) = Helm::global(cx) {
            subscriptions.push(cx.observe(&helm, |_, _, cx| cx.notify()));
        }
        if let Some(manager) = ConnectionManager::try_global(cx) {
            subscriptions.push(cx.observe(&manager, |_, _, cx| cx.notify()));
        }
        // Ages stay live.
        let ticker = cx.spawn(async move |this, cx| {
            loop {
                cx.background_executor()
                    .timer(Duration::from_secs(10))
                    .await;
                if this.update(cx, |_, cx| cx.notify()).is_err() {
                    break;
                }
            }
        });
        let olm = Olm::watch(&cluster, cx);
        let helm = Helm::watch(&cluster, cx);
        Self {
            cluster,
            tab: SubTab::Installed,
            filter,
            focus: cx.focus_handle(),
            scroll: UniformListScrollHandle::new(),
            selected: HashMap::new(),
            details_open: true,
            keys: Vec::new(),
            helm_namespace: None,
            instances: None,
            helm_resources: None,
            installed_rows: Vec::new(),
            plan_rows: Vec::new(),
            subscription_rows: Vec::new(),
            helm_rows: Vec::new(),
            _olm: olm,
            _helm: helm,
            _ticker: ticker,
            _subscriptions: subscriptions,
        }
    }

    pub fn cluster(&self) -> &ClusterId {
        &self.cluster
    }

    pub fn tab(&self) -> SubTab {
        self.tab
    }

    pub(crate) fn snapshot(&self, cx: &App) -> Option<std::sync::Arc<Snapshot>> {
        Olm::global(cx)?.read(cx).snapshot(&self.cluster, cx)
    }

    pub(crate) fn availability(&self, cx: &App) -> Availability {
        Olm::global(cx)
            .map(|o| o.read(cx).availability(&self.cluster, cx))
            .unwrap_or(Availability::NotConnected)
    }

    pub(crate) fn has_olm(&self, cx: &App) -> bool {
        !matches!(self.availability(cx), Availability::NoOlm)
    }

    pub(crate) fn read_only(&self, cx: &App) -> bool {
        ConnectionManager::try_global(cx).is_some_and(|m| m.read(cx).caps(&self.cluster).read_only)
    }

    pub(crate) fn cluster_name(&self, cx: &App) -> SharedString {
        ConnectionManager::try_global(cx)
            .map(|m| m.read(cx).display_name(&self.cluster))
            .unwrap_or_else(|| self.cluster.to_string().into())
    }

    pub(crate) fn query(&self, cx: &App) -> String {
        self.filter.read(cx).value().trim().to_lowercase()
    }

    /// Whether `haystack` matches every word of the filter.
    pub(crate) fn matches(query: &str, haystack: &str) -> bool {
        let haystack = haystack.to_lowercase();
        query.split_whitespace().all(|w| haystack.contains(w))
    }

    pub(crate) fn selected_key(&self) -> Option<&String> {
        self.selected.get(&self.tab)
    }

    /// The list row of a key (lists with group rows count those too).
    fn row_index(&self, key: &str) -> Option<usize> {
        match self.tab {
            SubTab::Plans => self.plan_rows.iter().position(|r| match r {
                plans::PlanRow::Plan(p) => plans::plan_key(p) == key,
                plans::PlanRow::Group(_) => false,
            }),
            _ => self.keys.iter().position(|k| k == key),
        }
    }

    pub(crate) fn select(&mut self, key: String, cx: &mut Context<Self>) {
        if let Some(index) = self.row_index(&key) {
            self.scroll.scroll_to_item(index, ScrollStrategy::Nearest);
        }
        self.selected.insert(self.tab, key);
        cx.notify();
    }

    fn move_selection(&mut self, delta: isize, cx: &mut Context<Self>) {
        if self.keys.is_empty() {
            return;
        }
        let current = self
            .selected_key()
            .and_then(|k| self.keys.iter().position(|x| x == k));
        let next = match current {
            None => 0,
            Some(i) => (i as isize + delta).clamp(0, self.keys.len() as isize - 1) as usize,
        };
        let key = self.keys[next].clone();
        self.select(key, cx);
    }

    pub(crate) fn set_tab(&mut self, tab: SubTab, window: &mut Window, cx: &mut Context<Self>) {
        if self.tab != tab {
            self.tab = tab;
            self.keys.clear();
            self.scroll = UniformListScrollHandle::new();
            let placeholder = tab.placeholder();
            self.filter.update(cx, |input, cx| {
                input.set_value("", window, cx);
                input.set_placeholder(placeholder, window, cx);
            });
        }
        self.focus.focus(window, cx);
        cx.notify();
    }

    /// Picks up a pending open (a tab and a row to select).
    fn check_pending(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Some(pending) = cx
            .try_global::<PendingOpens>()
            .and_then(|p| p.0.get(&self.cluster).cloned())
        else {
            return;
        };
        cx.default_global::<PendingOpens>().0.remove(&self.cluster);
        if let Some(tab) = pending.tab {
            self.set_tab(tab, window, cx);
        }
        if let Some(key) = pending.select {
            self.selected.insert(self.tab, key);
            self.details_open = true;
        }
    }

    /// The part of a sub-tab that takes the keys: the list and details, never the filter.
    pub(crate) fn focus_area(&self) -> gpui::Div {
        div()
            .key_context(self.tab.context())
            .track_focus(&self.focus)
            .flex()
            .flex_1()
            .min_h_0()
            .min_w_0()
    }

    fn render_toolbar(&self, cx: &mut Context<Self>) -> AnyElement {
        let colors = cx.colors().clone();
        let summary = self.summary(cx);
        let focused = false;
        let input = h_flex()
            .id("operators-filter")
            .flex_none()
            .w(u(230.0))
            .h(u(sizes::CONTROL))
            .px(u(8.0))
            .gap(u(7.0))
            .rounded(u(5.0))
            .bg(colors.input_background)
            .border_1()
            .border_color(if focused {
                colors.accent
            } else {
                colors.border
            })
            .text_size(u(12.5))
            .child(
                Icon::new(IconName::Search)
                    .size(12.0)
                    .color(colors.text_dim),
            )
            .child(
                div().flex_1().min_w_0().child(
                    Input::new(&self.filter)
                        .appearance(false)
                        .text_size(u(12.5)),
                ),
            );
        let cluster = self.cluster.clone();
        let show_hub = self.has_olm(cx) && self.snapshot(cx).is_some_and(|s| s.v0);
        h_flex()
            .flex_none()
            .h(u(40.0))
            .px(u(12.0))
            .gap(u(8.0))
            .border_b_1()
            .border_color(colors.border_variant)
            .overflow_hidden()
            .child(Icon::new(IconName::Blocks).size(14.0).color(colors.accent))
            .child(div().flex_none().font_weight(FontWeight::MEDIUM).child(
                if self.tab == SubTab::Helm {
                    "Helm releases"
                } else {
                    "Operators"
                },
            ))
            .child(div().text_color(colors.text_dim).child("·"))
            .child(
                div()
                    .flex_1()
                    .min_w_0()
                    .truncate()
                    .text_color(colors.text_dim)
                    .child(summary),
            )
            .child(input)
            .when(show_hub, |this| {
                this.child(
                    kubyl_ui::Button::new("browse-operatorhub")
                        .primary()
                        .icon(IconName::Store)
                        .label("Browse OperatorHub")
                        .on_click(move |_, window, cx| crate::hub::open(&cluster, window, cx)),
                )
            })
            .into_any_element()
    }

    fn summary(&self, cx: &App) -> String {
        if self.tab == SubTab::Helm {
            return self.helm_summary(cx);
        }
        let Some(snapshot) = self.snapshot(cx) else {
            return String::new();
        };
        let version = match (snapshot.v0, snapshot.v1) {
            (true, true) => "OLM v0 and v1",
            (true, false) => "OLM v0",
            (false, true) => "OLM v1",
            (false, false) => "no OLM",
        };
        match self.tab {
            SubTab::Installed => {
                let counts = crate::olm::join::Counts::of(&snapshot.operators);
                let mut parts = vec![
                    version.to_string(),
                    format!("{} installed", counts.installed),
                ];
                if counts.waiting > 0 {
                    parts.push(format!("{} waiting for approval", counts.waiting));
                }
                if counts.failing > 0 {
                    parts.push(format!("{} failing", counts.failing));
                }
                parts.join(" · ")
            }
            SubTab::Plans => format!(
                "{version} · {} install plans · {} waiting",
                snapshot.plans.len(),
                snapshot.pending_plans()
            ),
            SubTab::Subscriptions => {
                let failing = snapshot
                    .subscriptions
                    .iter()
                    .filter(|s| s.failure().is_some())
                    .count();
                let mut text =
                    format!("{version} · {} subscriptions", snapshot.subscriptions.len());
                if failing > 0 {
                    text.push_str(&format!(" · {failing} failing"));
                }
                text
            }
            SubTab::Extensions => format!(
                "{} ClusterExtensions · {} ClusterCatalogs",
                snapshot.extensions.len(),
                snapshot.cluster_catalogs.len()
            ),
            SubTab::Helm => unreachable!(),
        }
    }

    fn render_tabs(&self, window: &mut Window, cx: &mut Context<Self>) -> AnyElement {
        let colors = cx.colors().clone();
        let snapshot = self.snapshot(cx);
        let has_olm = self.has_olm(cx);
        let helm_count = Helm::global(cx)
            .and_then(|h| h.read(cx).snapshot(&self.cluster, cx))
            .map(|s| s.releases.len());
        let mut tabs: Vec<AnyElement> = Vec::new();
        let tab = |id: &'static str,
                   tab: SubTab,
                   icon: IconName,
                   label: &'static str,
                   badge: Option<(String, Option<gpui::Hsla>)>,
                   cx: &mut Context<Self>| {
            widgets::sub_tab(id, icon, label, badge, self.tab == tab, &colors)
                .on_click(cx.listener(move |this, _, window, cx| this.set_tab(tab, window, cx)))
                .into_any_element()
        };
        let v0 = snapshot.as_ref().is_some_and(|s| s.v0);
        tabs.push(tab(
            "operators-tab-installed",
            SubTab::Installed,
            IconName::Blocks,
            "Installed",
            snapshot
                .as_ref()
                .filter(|s| s.v0 && !s.loading)
                .map(|s| (s.operators.len().to_string(), None)),
            cx,
        ));
        if has_olm && v0 {
            let pending = snapshot.as_ref().map_or(0, |s| s.pending_plans());
            tabs.push(tab(
                "operators-tab-plans",
                SubTab::Plans,
                IconName::ListChecks,
                "Install plans",
                (pending > 0).then(|| (format!("{pending} pending"), Some(colors.yellow))),
                cx,
            ));
            tabs.push(tab(
                "operators-tab-subscriptions",
                SubTab::Subscriptions,
                IconName::List,
                "Subscriptions",
                snapshot
                    .as_ref()
                    .filter(|s| !s.loading)
                    .map(|s| (s.subscriptions.len().to_string(), None)),
                cx,
            ));
        }
        tabs.push(tab(
            "operators-tab-helm",
            SubTab::Helm,
            IconName::Anchor,
            "Helm releases",
            helm_count.map(|n| (n.to_string(), None)),
            cx,
        ));
        if snapshot.as_ref().is_some_and(|s| s.v1) {
            tabs.push(tab(
                "operators-tab-extensions",
                SubTab::Extensions,
                IconName::Blocks,
                "Extensions",
                snapshot
                    .as_ref()
                    .map(|s| (s.extensions.len().to_string(), None)),
                cx,
            ));
        }
        // A tab that disappeared (OLM removed): back to Installed.
        let valid = match self.tab {
            SubTab::Plans | SubTab::Subscriptions => has_olm && v0,
            SubTab::Extensions => snapshot.as_ref().is_some_and(|s| s.v1),
            _ => true,
        };
        if !valid && snapshot.is_some() {
            self.clone_tab_reset(window, cx);
        }
        h_flex()
            .flex_none()
            .h(u(34.0))
            .px(u(8.0))
            .gap(u(2.0))
            .border_b_1()
            .border_color(colors.border_variant)
            .children(tabs)
            .into_any_element()
    }

    fn clone_tab_reset(&self, window: &mut Window, cx: &mut Context<Self>) {
        let _ = window;
        cx.defer_in(window, |this, window, cx| {
            this.set_tab(SubTab::Installed, window, cx)
        });
    }

    pub(crate) fn hints(&self, cx: &App) -> Vec<(SharedString, SharedString)> {
        let read_only = self.read_only(cx);
        let mut hints: Vec<(SharedString, SharedString)> = ActionRegistry::global(cx)
            .hints(self.tab.context())
            .into_iter()
            .filter(|(_, hint)| !read_only || !WRITE_HINTS.contains(&hint.as_ref()))
            .collect();
        hints.push(("/".into(), "Filter".into()));
        hints
    }

    fn bind_actions(&self, this: gpui::Div, cx: &mut Context<Self>) -> gpui::Div {
        this.on_action(cx.listener(|view, _: &FocusFilter, window, cx| {
            let focus = view.filter.read(cx).focus_handle(cx);
            focus.focus(window, cx);
        }))
        .on_action(cx.listener(|view, _: &BlurFilter, window, cx| {
            view.focus.focus(window, cx);
        }))
        .on_action(cx.listener(|view, _: &SelectNext, _, cx| view.move_selection(1, cx)))
        .on_action(cx.listener(|view, _: &SelectPrevious, _, cx| view.move_selection(-1, cx)))
        .on_action(cx.listener(|view, _: &Details, _, cx| {
            view.details_open = !view.details_open || view.selected_key().is_none();
            if view.selected_key().is_none()
                && let Some(first) = view.keys.first().cloned()
            {
                view.selected.insert(view.tab, first);
            }
            cx.notify();
        }))
        .on_action(cx.listener(|view, _: &Approve, window, cx| view.approve(window, cx)))
        .on_action(cx.listener(|view, _: &ReviewChanges, window, cx| view.review(window, cx)))
        .on_action(cx.listener(|view, _: &CreateInstance, window, cx| view.create(window, cx)))
        .on_action(cx.listener(|view, _: &ViewYaml, window, cx| view.yaml(false, window, cx)))
        .on_action(cx.listener(|view, _: &EditYaml, window, cx| view.yaml(true, window, cx)))
        .on_action(cx.listener(|view, _: &Uninstall, window, cx| view.uninstall(window, cx)))
        .on_action(
            cx.listener(|view, _: &OpenRelease, window, cx| view.open_release(None, window, cx)),
        )
        .on_action(cx.listener(|view, _: &ReleaseValues, window, cx| {
            view.open_release(Some(crate::release::ReleaseTab::Values), window, cx)
        }))
        .on_action(cx.listener(|view, _: &ReleaseManifest, window, cx| {
            view.open_release(Some(crate::release::ReleaseTab::Manifest), window, cx)
        }))
        .on_action(cx.listener(|view, _: &ReleaseHistory, window, cx| {
            view.open_release(Some(crate::release::ReleaseTab::History), window, cx)
        }))
        .on_action(cx.listener(|view, _: &HelmCommands, window, cx| view.helm_commands(window, cx)))
        .on_action(cx.listener(|view, _: &NewExtension, window, cx| view.new_extension(window, cx)))
    }

    fn approve(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let plan = match self.tab {
            SubTab::Installed => self
                .selected_operator(cx)
                .and_then(|o| o.approvable().cloned()),
            SubTab::Plans => self.selected_plan(cx).filter(|p| p.needs_approval()),
            _ => None,
        };
        if let Some(plan) = plan {
            crate::dialogs::open_review(self.cluster.clone(), plan, window, cx);
        }
    }

    fn review(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let plan = match self.tab {
            SubTab::Installed => self.selected_operator(cx).and_then(|o| o.plan.clone()),
            SubTab::Plans => self.selected_plan(cx),
            _ => None,
        };
        if let Some(plan) = plan {
            crate::dialogs::open_review(self.cluster.clone(), plan, window, cx);
        }
    }

    fn create(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.read_only(cx) {
            return;
        }
        if let Some(csv) = self.selected_operator(cx).and_then(|o| o.csv.clone()) {
            crate::dialogs::open_create_picker(self.cluster.clone(), csv, window, cx);
        }
    }

    fn yaml(&mut self, edit: bool, window: &mut Window, cx: &mut Context<Self>) {
        let target = match self.tab {
            SubTab::Installed => self.selected_operator(cx).and_then(|o| {
                let csv = o.csv.as_ref()?;
                Some(ResourceRef::object(
                    self.cluster.clone(),
                    crate::olm::model::csvs(),
                    Some(csv.namespace.clone()),
                    csv.name.clone(),
                ))
            }),
            SubTab::Plans => self.selected_plan(cx).map(|p| {
                ResourceRef::object(
                    self.cluster.clone(),
                    crate::olm::model::install_plans(),
                    Some(p.namespace.clone()),
                    p.name.clone(),
                )
            }),
            SubTab::Subscriptions => self.selected_subscription(cx).map(|s| {
                ResourceRef::object(
                    self.cluster.clone(),
                    crate::olm::model::subscriptions(),
                    Some(s.namespace.clone()),
                    s.name.clone(),
                )
            }),
            SubTab::Extensions => self.selected_key().and_then(|k| {
                let name = k.strip_prefix("ext:")?;
                Some(ResourceRef::object(
                    self.cluster.clone(),
                    crate::olm::v1::cluster_extensions(),
                    None,
                    name.to_string(),
                ))
            }),
            SubTab::Helm => None,
        };
        let _ = edit;
        if let Some(target) = target {
            window.dispatch_action(
                Box::new(OpenView(ViewRequest::for_resource(ViewKind::Yaml, target))),
                cx,
            );
        }
    }

    fn uninstall(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.read_only(cx) {
            return;
        }
        let operator = match self.tab {
            SubTab::Installed => self.selected_operator(cx),
            SubTab::Subscriptions => self.selected_subscription(cx).and_then(|s| {
                self.snapshot(cx)?
                    .operators
                    .iter()
                    .find(|o| o.subscription.as_ref().is_some_and(|x| x.key() == s.key()))
                    .cloned()
            }),
            _ => None,
        };
        if let Some(operator) = operator {
            crate::dialogs::open_uninstall(self.cluster.clone(), operator, window, cx);
        }
    }

    fn new_extension(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.read_only(cx) {
            return;
        }
        let text = crate::olm::v1::extension_template("my-operator", "my-operator");
        kubyl_yaml::open_draft(
            ResourceRef::list(self.cluster.clone(), crate::olm::v1::cluster_extensions(), None),
            text,
            Some(
                "OLM v1 template: set the package, then apply. The installer service account needs the bundle's permissions."
                    .into(),
            ),
            window,
            cx,
        );
    }
}

impl Focusable for OperatorsView {
    fn focus_handle(&self, _: &App) -> FocusHandle {
        self.focus.clone()
    }
}

impl TabView for OperatorsView {
    fn tab_title(&self, cx: &App) -> SharedString {
        let base = if self.has_olm(cx) {
            "Operators"
        } else {
            "Helm Releases"
        };
        let active =
            ActiveContext::global(cx).cluster.as_ref().map(|c| &c.id) == Some(&self.cluster);
        if active {
            base.into()
        } else {
            format!("{base} · {}", self.cluster_name(cx)).into()
        }
    }

    fn tab_icon(&self, cx: &App) -> Option<SharedString> {
        Some(if self.has_olm(cx) {
            IconName::Blocks.path()
        } else {
            IconName::Anchor.path()
        })
    }

    fn tab_dot(&self, cx: &App) -> Option<gpui::Hsla> {
        let active = ActiveContext::global(cx)
            .cluster
            .as_ref()
            .map(|c| c.id.clone());
        (active.as_ref() != Some(&self.cluster))
            .then(|| ConnectionManager::try_global(cx).map(|m| m.read(cx).color(&self.cluster, cx)))
            .flatten()
    }

    fn view_request(&self, _: &App) -> Option<ViewRequest> {
        Some(request(&self.cluster))
    }
}

impl Render for OperatorsView {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        self.check_pending(window, cx);
        let colors: Colors = cx.colors().clone();
        let toolbar = self.render_toolbar(cx);
        let tabs = self.render_tabs(window, cx);
        let availability = self.availability(cx);
        let body: AnyElement = if self.tab.needs_olm()
            && let Some(state) = states::blocking(availability, &self.snapshot(cx))
        {
            self.keys.clear();
            self.render_state(state, cx)
        } else {
            match self.tab {
                SubTab::Installed => self.render_installed(window, cx),
                SubTab::Plans => self.render_plans(window, cx),
                SubTab::Subscriptions => self.render_subscriptions(window, cx),
                SubTab::Helm => self.render_helm(window, cx),
                SubTab::Extensions => self.render_extensions(window, cx),
            }
        };
        let hints = self.hints(cx);
        v_flex()
            .key_context(VIEW_CONTEXT)
            .size_full()
            .bg(colors.background)
            .text_color(colors.text)
            .font_family(fonts::UI)
            .text_size(u(sizes::UI_FONT))
            .map(|this| self.bind_actions(this, cx))
            .child(toolbar)
            .child(tabs)
            .child(div().flex_1().min_h_0().flex().child(body))
            .child(kubyl_ui::KeyHints::new(hints))
    }
}
