//! The Cluster Updates tab (board 8): one per cluster. The current version and channel, the
//! update path, pre-flight checks, a running update's progress, the control plane and pools,
//! add-ons and history; writes open [`confirm`] first.

mod cards;
pub mod confirm;
pub mod widgets;

use std::collections::HashSet;
use std::time::Duration;

use gpui::{
    AnyElement, App, AppContext as _, Context, FocusHandle, Focusable, FontWeight, IntoElement,
    KeyBinding, Render, ScrollHandle, SharedString, Subscription, Task, Window, actions, div,
    prelude::*,
};
use kubyl_core::actions::OpenView;
use kubyl_core::{
    ActionRegistry, ActionSpec, ActiveContext, ClusterId, Gvr, ResourceRef, TabView, ViewKind,
    ViewRegistry, ViewRequest,
};
use kubyl_kube::ConnectionManager;
use kubyl_ui::{ActiveColors, Button, Colors, Icon, IconName, fonts, h_flex, sizes, u, v_flex};

use crate::check::Check;
use crate::model::{Scope, Status};
use crate::provider::ProviderError;
use crate::service::{ReadState, UpdateState, Updates, UpdatesLease};

/// The view's key context.
pub const CONTEXT: &str = "UpdatesView";

actions!(
    updates,
    [
        /// Opens the active cluster's Cluster Updates tab.
        ShowUpdates,
        /// Reads the provider again.
        Recheck,
        /// Runs the pre-flight checks again.
        RerunPreflight,
        /// Starts an update to the selected version (after the summary).
        StartUpdate,
        /// Selects the next version of the update path.
        SelectNext,
        /// Selects the previous version of the update path.
        SelectPrevious,
    ]
);

/// Hints of writing actions, hidden on read-only clusters.
const WRITE_HINTS: &[&str] = &["Update…"];

pub(crate) fn request(cluster: &ClusterId) -> ViewRequest {
    ViewRequest::for_resource(
        ViewKind::Updates,
        ResourceRef::list(cluster.clone(), Gvr::new("", "", ""), None),
    )
}

pub(crate) fn init(cx: &mut App) {
    ViewRegistry::register(cx, ViewKind::Updates, |request, window, cx| {
        let cluster = request.target.as_ref()?.cluster.clone();
        Some(Box::new(cx.new(|cx| UpdatesView::new(cluster, window, cx))))
    });
    ActionRegistry::register(
        cx,
        ActionSpec::new("Updates: Show Cluster Updates", ShowUpdates),
    );
    for (spec, keys) in [
        (
            ActionSpec::new("Updates: Re-check", Recheck).hint("Re-check"),
            "r",
        ),
        (
            ActionSpec::new("Updates: Re-run Pre-flight Checks", RerunPreflight)
                .hint("Re-run checks"),
            "shift-r",
        ),
        (
            ActionSpec::new("Updates: Update…", StartUpdate).hint("Update…"),
            "u",
        ),
    ] {
        ActionRegistry::register(cx, spec.bind(keys, Some(CONTEXT)));
    }
    cx.bind_keys([
        KeyBinding::new("down", SelectNext, Some(CONTEXT)),
        KeyBinding::new("up", SelectPrevious, Some(CONTEXT)),
        KeyBinding::new("j", SelectNext, Some(CONTEXT)),
        KeyBinding::new("k", SelectPrevious, Some(CONTEXT)),
    ]);
    cx.on_action(|_: &ShowUpdates, cx| {
        let Some(cluster) = ActiveContext::global(cx)
            .cluster
            .as_ref()
            .map(|c| c.id.clone())
        else {
            return;
        };
        cx.defer(move |cx| {
            let window = cx.active_window().or_else(|| cx.windows().first().copied());
            if let Some(window) = window {
                window
                    .update(cx, |_, window, cx| {
                        window.dispatch_action(Box::new(OpenView(request(&cluster))), cx)
                    })
                    .ok();
            }
        });
    });
}

pub struct UpdatesView {
    pub(crate) cluster: ClusterId,
    focus: FocusHandle,
    scroll: ScrollHandle,
    /// The target selected in the update path.
    pub(crate) selected: Option<String>,
    /// Read-only providers: the version the checks run against.
    pub(crate) check_target: Option<String>,
    /// Expanded pre-flight checks (by id).
    pub(crate) expanded: HashSet<&'static str>,
    pub(crate) show_passed: bool,
    pub(crate) show_all_components: bool,
    /// Targets whose checks this view asked for (once each; Re-run asks again).
    asked: HashSet<(String, String)>,
    _lease: Option<UpdatesLease>,
    _ticker: Task<()>,
    _subscriptions: Vec<Subscription>,
}

impl UpdatesView {
    pub fn new(cluster: ClusterId, _: &mut Window, cx: &mut Context<Self>) -> Self {
        let mut subscriptions = Vec::new();
        if let Some(updates) = Updates::global(cx) {
            subscriptions.push(cx.observe(&updates, |this, _, cx| {
                this.ask_preflight(cx);
                cx.notify();
            }));
        }
        if let Some(manager) = ConnectionManager::try_global(cx) {
            subscriptions.push(cx.observe(&manager, |_, _, cx| cx.notify()));
        }
        if let Some(olm) = kubyl_operators::Olm::global(cx) {
            // The operators check follows installs and removals.
            subscriptions.push(cx.observe(&olm, |_, _, cx| cx.notify()));
        }
        // Ages and "ran 2m ago" stay live.
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
        let lease = Updates::watch(&cluster, cx);
        let mut view = Self {
            cluster,
            focus: cx.focus_handle(),
            scroll: ScrollHandle::new(),
            selected: None,
            check_target: None,
            expanded: HashSet::new(),
            show_passed: false,
            show_all_components: false,
            asked: HashSet::new(),
            _lease: lease,
            _ticker: ticker,
            _subscriptions: subscriptions,
        };
        view.ask_preflight(cx);
        view
    }

    pub fn cluster(&self) -> &ClusterId {
        &self.cluster
    }

    pub(crate) fn status(&self, cx: &App) -> Option<std::sync::Arc<Status>> {
        Updates::global(cx)?.read(cx).status(&self.cluster)
    }

    pub(crate) fn read_only(&self, cx: &App) -> bool {
        ConnectionManager::try_global(cx).is_some_and(|m| m.read(cx).caps(&self.cluster).read_only)
    }

    pub(crate) fn cluster_name(&self, cx: &App) -> SharedString {
        ConnectionManager::try_global(cx)
            .map(|m| m.read(cx).display_name(&self.cluster))
            .unwrap_or_else(|| self.cluster.to_string().into())
    }

    /// The target the checks run against: the selected one, the suggestion, or (read-only
    /// providers without targets) the next minor.
    pub(crate) fn check_target(&self, status: &Status) -> Option<String> {
        if status.targets.is_empty() {
            return self.check_target.clone().or_else(|| {
                let base = status
                    .current
                    .kubernetes
                    .as_deref()
                    .unwrap_or(&status.current.version);
                crate::version::next_minor(base)
            });
        }
        self.selected
            .as_ref()
            .filter(|v| status.target(v).is_some())
            .cloned()
            .or_else(|| status.suggested().map(|t| t.version.clone()))
            .or_else(|| status.targets.first().map(|t| t.version.clone()))
    }

    /// Runs the checks of the current target once it's known (once per target and version).
    fn ask_preflight(&mut self, cx: &mut Context<Self>) {
        let Some(updates) = Updates::global(cx) else {
            return;
        };
        let Some(status) = updates.read(cx).status(&self.cluster) else {
            return;
        };
        // Nothing can start while an update runs.
        if status.updating() {
            return;
        }
        let Some(target) = self.check_target(&status) else {
            return;
        };
        let key = (target.clone(), status.current.version.clone());
        if self.asked.contains(&key) || updates.read(cx).preflight(&self.cluster, &target).is_some()
        {
            return;
        }
        self.asked.insert(key);
        let cluster = self.cluster.clone();
        // Not while the service is notifying us.
        cx.defer(move |cx| {
            updates.update(cx, |updates, cx| {
                updates.run_preflight(&cluster, &target, cx)
            });
        });
    }

    fn recheck(&mut self, cx: &mut Context<Self>) {
        let cluster = self.cluster.clone();
        if let Some(updates) = Updates::global(cx) {
            updates.update(cx, |updates, cx| updates.read(&cluster, cx));
        }
    }

    pub(crate) fn rerun_preflight(&mut self, cx: &mut Context<Self>) {
        let Some(status) = self.status(cx) else {
            return;
        };
        let Some(target) = self.check_target(&status) else {
            return;
        };
        let cluster = self.cluster.clone();
        if let Some(updates) = Updates::global(cx) {
            updates.update(cx, |updates, cx| {
                updates.run_preflight(&cluster, &target, cx)
            });
        }
    }

    pub(crate) fn select(&mut self, version: String, cx: &mut Context<Self>) {
        self.selected = Some(version);
        self.ask_preflight(cx);
        cx.notify();
    }

    fn move_selection(&mut self, delta: isize, cx: &mut Context<Self>) {
        let Some(status) = self.status(cx) else {
            return;
        };
        if status.targets.is_empty() {
            return;
        }
        let current = self
            .check_target(&status)
            .and_then(|t| status.targets.iter().position(|x| x.version == t))
            .unwrap_or(0);
        let next = (current as isize + delta).clamp(0, status.targets.len() as isize - 1) as usize;
        self.select(status.targets[next].version.clone(), cx);
    }

    /// The checks of `target`, if they ran.
    pub(crate) fn checks(&self, target: &str, cx: &mut App) -> Option<Vec<Check>> {
        let updates = Updates::global(cx)?;
        let cluster = self.cluster.clone();
        updates.update(cx, |updates, cx| updates.checks(&cluster, target, cx))
    }

    /// Opens the summary of a write.
    pub(crate) fn confirm(
        &mut self,
        scope: Scope,
        target: String,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if self.read_only(cx) {
            return;
        }
        let (Some(updates), Some(status)) = (Updates::global(cx), self.status(cx)) else {
            return;
        };
        let Some(provider) = updates.read(cx).provider(&self.cluster) else {
            return;
        };
        let name = self.cluster_name(cx).to_string();
        match provider.plan(&status, &scope, &target, &name) {
            Ok(plan) => {
                let checks = match scope {
                    Scope::Channel(_) => None,
                    _ => self.checks(&target, cx),
                };
                confirm::open(
                    self.cluster.clone(),
                    provider.kind(),
                    plan,
                    checks,
                    window,
                    cx,
                );
            }
            Err(message) => {
                kubyl_core::NotificationCenter::push(cx, kubyl_core::Notification::error(message))
            }
        }
    }

    /// `u`: the main update of the selected target.
    fn start_selected(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Some(status) = self.status(cx) else {
            return;
        };
        let Some(target) = self.check_target(&status) else {
            return;
        };
        let scope = if status.writes.control_plane {
            Scope::ControlPlane
        } else if status.writes.pools {
            Scope::AllPools
        } else {
            return;
        };
        self.confirm(scope, target, window, cx);
    }

    fn hints(&self, blocked: bool, cx: &App) -> Vec<(SharedString, SharedString)> {
        if blocked {
            return Vec::new();
        }
        let status = self.status(cx);
        // Writes are hidden on read-only clusters and where the provider can't write.
        let writable = !self.read_only(cx)
            && status
                .as_ref()
                .is_some_and(|s| s.writes.control_plane || s.writes.pools);
        let mut hints: Vec<(SharedString, SharedString)> = ActionRegistry::global(cx)
            .hints(CONTEXT)
            .into_iter()
            .filter(|(_, hint)| writable || !WRITE_HINTS.contains(&hint.as_ref()))
            .collect();
        if status.is_some_and(|s| s.targets.len() > 1) {
            hints.push(("↑↓".into(), "Select version".into()));
        }
        hints
    }

    fn render_header(
        &self,
        subtitle: String,
        updating: bool,
        colors: &Colors,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let reading = Updates::global(cx).is_some_and(|u| u.read(cx).is_reading(&self.cluster));
        h_flex()
            .flex_none()
            .gap(u(10.0))
            .px(u(18.0))
            .pt(u(14.0))
            .pb(u(10.0))
            .child(
                div()
                    .flex_none()
                    .text_size(u(18.0))
                    .font_weight(FontWeight::SEMIBOLD)
                    .child("Cluster updates"),
            )
            .when(updating, |this| {
                this.child(
                    h_flex()
                        .flex_none()
                        .gap(u(4.0))
                        .px(u(7.0))
                        .h(u(20.0))
                        .rounded(u(4.0))
                        .bg(colors.accent.opacity(0.15))
                        .text_size(u(11.5))
                        .text_color(colors.accent)
                        .child(
                            Icon::new(IconName::RefreshCw)
                                .size(11.0)
                                .color(colors.accent),
                        )
                        .child("updating"),
                )
            })
            .child(
                div()
                    .flex_1()
                    .min_w_0()
                    .truncate()
                    .text_size(u(12.0))
                    .text_color(colors.text_dim)
                    .child(subtitle),
            )
            .child(
                Button::new("updates-recheck")
                    .ghost()
                    .icon(IconName::RefreshCw)
                    .label(if reading { "Checking…" } else { "Re-check" })
                    .on_click(cx.listener(|this, _, _, cx| {
                        this.recheck(cx);
                        this.rerun_preflight(cx);
                    })),
            )
            .into_any_element()
    }

    /// A full-page state (not connected, detecting, a read that failed).
    fn render_state(
        &self,
        icon: IconName,
        title: String,
        text: String,
        colors: &Colors,
    ) -> AnyElement {
        v_flex()
            .flex_1()
            .items_center()
            .justify_center()
            .gap(u(8.0))
            .p(u(32.0))
            .child(Icon::new(icon).size(28.0).color(colors.text_dim))
            .child(div().font_weight(FontWeight::MEDIUM).child(title))
            .child(
                div()
                    .max_w(u(560.0))
                    .text_size(u(12.5))
                    .text_color(colors.text_muted)
                    .text_center()
                    .child(text),
            )
            .into_any_element()
    }
}

/// What a failed read says (never an empty state): a 403 names the verb and resource.
fn failure_text(err: &ProviderError) -> (String, String) {
    match err {
        ProviderError::Forbidden { verb, resource } => (
            "You can't read this cluster's update state".into(),
            format!("Missing permission: {verb} {resource}. Ask for a role that allows it."),
        ),
        ProviderError::Credentials { message, command } => (
            "Credentials aren't available".into(),
            match command {
                Some(command) => format!("{message} Run: {command}"),
                None => message.clone(),
            },
        ),
        other => ("Couldn't read the update state".into(), other.to_string()),
    }
}

impl Focusable for UpdatesView {
    fn focus_handle(&self, _: &App) -> FocusHandle {
        self.focus.clone()
    }
}

impl TabView for UpdatesView {
    fn tab_title(&self, cx: &App) -> SharedString {
        let active =
            ActiveContext::global(cx).cluster.as_ref().map(|c| &c.id) == Some(&self.cluster);
        if active {
            "Cluster Updates".into()
        } else {
            format!("Cluster Updates · {}", self.cluster_name(cx)).into()
        }
    }

    fn tab_icon(&self, _: &App) -> Option<SharedString> {
        Some(IconName::ArrowUp.path())
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

impl Render for UpdatesView {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let colors = cx.colors().clone();
        let state = Updates::global(cx)
            .map(|u| u.read(cx).state(&self.cluster, cx))
            .unwrap_or(UpdateState::NotConnected);
        let name = self.cluster_name(cx);
        let (header, body, blocked): (AnyElement, AnyElement, bool) = match state {
            UpdateState::NotConnected => (
                self.render_header(name.to_string(), false, &colors, cx),
                self.render_state(
                    IconName::Cloud,
                    "Not connected".into(),
                    format!("Connect to {name} to see its version and updates."),
                    &colors,
                ),
                true,
            ),
            UpdateState::Detecting => (
                self.render_header(name.to_string(), false, &colors, cx),
                self.render_state(
                    IconName::RefreshCw,
                    "Looking at the cluster…".into(),
                    "Kubyl picks the update provider once discovery has finished.".into(),
                    &colors,
                ),
                true,
            ),
            UpdateState::Known {
                detected,
                read,
                last,
                ..
            } => {
                let status = match (&read, last) {
                    (ReadState::Ready(status), _) => Some(status.clone()),
                    (_, Some(last)) => Some(last),
                    _ => None,
                };
                let subtitle = match &status {
                    Some(status) => format!("{name} · provider {}", status.provider),
                    None => format!("{name} · provider {}", detected.kind.label()),
                };
                let updating = status.as_ref().is_some_and(|s| s.updating());
                let header = self.render_header(subtitle, updating, &colors, cx);
                match (status, read) {
                    (Some(status), read) => {
                        let stale = match read {
                            ReadState::Failed(err) => Some(err),
                            _ => None,
                        };
                        (
                            header,
                            self.render_status(&status, stale, window, cx),
                            false,
                        )
                    }
                    (None, ReadState::Failed(err)) => {
                        let (title, text) = failure_text(&err);
                        (
                            header,
                            self.render_state(IconName::TriangleAlert, title, text, &colors),
                            true,
                        )
                    }
                    (None, _) => (
                        header,
                        self.render_state(
                            IconName::RefreshCw,
                            format!("Reading {}…", detected.kind.label()),
                            detected.reason.clone(),
                            &colors,
                        ),
                        true,
                    ),
                }
            }
        };
        let hints = self.hints(blocked, cx);
        v_flex()
            .key_context(CONTEXT)
            .track_focus(&self.focus)
            .size_full()
            .bg(colors.background)
            .text_color(colors.text)
            .font_family(fonts::UI)
            .text_size(u(sizes::UI_FONT))
            .on_action(cx.listener(|this, _: &Recheck, _, cx| this.recheck(cx)))
            .on_action(cx.listener(|this, _: &RerunPreflight, _, cx| this.rerun_preflight(cx)))
            .on_action(
                cx.listener(|this, _: &StartUpdate, window, cx| this.start_selected(window, cx)),
            )
            .on_action(cx.listener(|this, _: &SelectNext, _, cx| this.move_selection(1, cx)))
            .on_action(cx.listener(|this, _: &SelectPrevious, _, cx| this.move_selection(-1, cx)))
            .child(header)
            .child(
                div()
                    .id("updates-body")
                    .flex_1()
                    .min_h_0()
                    .overflow_y_scroll()
                    .track_scroll(&self.scroll)
                    .child(body),
            )
            .child(kubyl_ui::KeyHints::new(hints))
    }
}

impl UpdatesView {
    fn render_status(
        &mut self,
        status: &Status,
        stale: Option<ProviderError>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let colors = cx.colors().clone();
        let read_only = self.read_only(cx);
        let mut page = v_flex().px(u(18.0)).pb(u(18.0)).gap(u(14.0));
        if let Some(err) = stale {
            page = page.child(cards::banner(
                true,
                "Showing the last read".into(),
                failure_text(&err).1,
                None,
                None,
                "stale",
                &colors,
            ));
        }
        for (ix, note) in status.notes.iter().enumerate() {
            page = page.child(cards::banner(
                note.warning,
                note.title.clone(),
                note.text.clone(),
                note.command.clone(),
                note.url.clone(),
                ix,
                &colors,
            ));
        }
        if read_only && status.writes.any() {
            page = page.child(cards::banner(
                false,
                "This cluster is read-only in Kubyl".into(),
                "Updates and channel changes are hidden. Turn off read-only in the cluster's settings to update it.".into(),
                None,
                None,
                "read-only",
                &colors,
            ));
        }
        if let Some(progress) = &status.progress {
            page = page.child(cards::progress(status, progress, &colors));
        }
        let target = self.check_target(status);
        let updating = status.updating();
        // While an update runs, what it rolls through comes first; nothing new can start, so
        // no pre-flight.
        if updating {
            if !status.components.is_empty() {
                page = page.child(cards::components_card(self, status, cx));
            }
            if !status.pools.is_empty() {
                page = page.child(cards::pools_card(
                    self,
                    status,
                    target.as_deref(),
                    read_only,
                    window,
                    cx,
                ));
            }
        }
        page = page.child(
            h_flex()
                .items_start()
                .gap(u(12.0))
                .child(cards::version_card(self, status, read_only, window, cx))
                .child(cards::path_card(
                    self,
                    status,
                    target.as_deref(),
                    read_only,
                    cx,
                )),
        );
        if let Some(target) = target.as_ref().filter(|_| !updating) {
            page = page.child(cards::preflight_card(self, status, target, cx));
        }
        if !updating && !status.components.is_empty() {
            page = page.child(cards::components_card(self, status, cx));
        }
        if !updating && !status.pools.is_empty() {
            page = page.child(cards::pools_card(
                self,
                status,
                target.as_deref(),
                read_only,
                window,
                cx,
            ));
        }
        if !status.addons.is_empty() {
            page = page.child(cards::addons_card(self, status, read_only, cx));
        }
        if !status.history.is_empty() {
            page = page.child(cards::history_card(status, &colors));
        }
        if !status.docs.is_empty() {
            page = page.child(cards::docs_card(status, &colors));
        }
        page.into_any_element()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::detect::Detected;
    use crate::model::ProviderKind;
    use gpui::{Entity, TestAppContext};

    pub(crate) fn cluster_version() -> serde_json::Value {
        serde_json::json!({
            "spec": {"channel": "stable-4.17"},
            "status": {
                "desired": {"version": "4.17.8", "channels": ["stable-4.17", "stable-4.18"]},
                "history": [{"state": "Completed", "version": "4.17.8", "startedTime": "2026-08-30T10:00:00Z", "completionTime": "2026-08-30T11:12:00Z"}],
                "availableUpdates": [{"version": "4.17.9"}, {"version": "4.17.12"}],
                "conditionalUpdates": [{"release": {"version": "4.17.13"},
                    "risks": [{"name": "ExampleStorageDriverRegression", "message": "Volumes may fail to attach."}],
                    "conditions": [{"type": "Recommended", "status": "False"}]}],
                "conditions": [{"type": "Available", "status": "True"}, {"type": "Progressing", "status": "False"}]
            }
        })
    }

    pub(crate) fn setup(cx: &mut TestAppContext) -> Entity<Updates> {
        let dir = tempfile::tempdir().unwrap();
        cx.update(|cx| {
            kubyl_core::init(cx);
            kubyl_settings::init_with_dir(cx, dir.path());
            kubyl_ui::init(cx);
            kubyl_resources::init(cx);
            let updates = Updates::install(false, cx);
            init(cx);
            updates
        })
    }

    /// The tab suggests the newest recommended update, and up/down walk the update path.
    #[gpui::test]
    fn picks_the_suggested_target_and_moves_the_selection(cx: &mut TestAppContext) {
        let updates = setup(cx);
        let cluster = ClusterId::new("ocp");
        updates.update(cx, |u, cx| {
            u.insert_for_test(
                &cluster,
                Detected {
                    kind: ProviderKind::OpenShift,
                    reason: "test".into(),
                },
                crate::openshift::parse_cluster_version(&cluster_version()),
                cx,
            )
        });
        let slot: std::rc::Rc<std::cell::RefCell<Option<Entity<UpdatesView>>>> = Default::default();
        let (_root, cx) = cx.add_window_view({
            let slot = slot.clone();
            let cluster = cluster.clone();
            move |window, cx| {
                let view = cx.new(|cx| UpdatesView::new(cluster, window, cx));
                *slot.borrow_mut() = Some(view.clone());
                gpui_component::Root::new(view, window, cx)
            }
        });
        let view = slot.borrow().clone().unwrap();
        cx.run_until_parked();
        view.update_in(cx, |view, _, cx| {
            let status = view.status(cx).unwrap();
            // Newest first: 4.17.13 (conditional), 4.17.12 (recommended), 4.17.9.
            assert_eq!(view.check_target(&status).as_deref(), Some("4.17.12"));
            view.move_selection(-1, cx);
            assert_eq!(view.check_target(&status).as_deref(), Some("4.17.13"));
            view.move_selection(5, cx);
            assert_eq!(view.check_target(&status).as_deref(), Some("4.17.9"));
            // No writes happen without a provider (and none on read-only clusters).
            assert!(!view.read_only(cx));
        });
    }
}
