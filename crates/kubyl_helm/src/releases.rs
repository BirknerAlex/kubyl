//! The Helm releases list (board 7, phase 12): the releases of a cluster with a details pane
//! (summary, history, resources). The Operators tab renders it on its "Helm releases" sub-tab;
//! values, manifest and notes open in the release's own tab ([`crate::release`]).
//!
//! The view owns its selection, keys and details; the embedding view owns the filter and passes
//! the query in ([`ReleasesView::set_query`]).

use std::ops::Range;
use std::sync::Arc;

use gpui::{
    AnyElement, App, AppContext as _, Context, Entity, FocusHandle, Focusable, FontWeight,
    IntoElement, Render, ScrollStrategy, Subscription, Task, UniformListScrollHandle, Window,
    actions, div, prelude::*, px, uniform_list,
};
use gpui_component::button::{Button as MenuButton, ButtonVariants as _};
use gpui_component::menu::{DropdownMenu as _, PopupMenuItem};
use kubyl_core::actions::OpenView;
use kubyl_core::{
    ActionRegistry, ActionSpec, ClusterId, ColumnDef, ColumnWidth, Gvk, ResourceRef, Tone,
    ViewRegistry, ViewRequest, spawn_kube,
};
use kubyl_kube::ConnectionManager;
use kubyl_ui::{ActiveColors, Button, Chip, Colors, Icon, IconName, fonts, h_flex, u, v_flex};

use crate::present::{self, ManifestObject};
use crate::release::ReleaseTab;
use crate::service::{Helm, HelmLease, ReleaseRow, Snapshot, SummaryState};
use crate::widgets;

/// Key context of the list (keys work there, never in the embedding view's filter).
pub const CONTEXT: &str = "HelmReleases";

const ROW_HEIGHT: f32 = 32.0;

actions!(
    helm_releases,
    [
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
        /// Upgrades the selected release.
        Upgrade,
        /// Rolls the selected release back.
        RollBack,
        /// Uninstalls the selected release.
        Uninstall,
        /// Installs a chart (into the list's cluster).
        Install,
        /// Asks the agent about the selected release.
        AskAgent,
    ]
);

pub(crate) fn init(cx: &mut App) {
    for (spec, keys) in [
        (
            ActionSpec::new("Helm: Open Release", OpenRelease).hint("Open release"),
            "enter",
        ),
        (
            ActionSpec::new("Helm: Values", ReleaseValues).hint("Values"),
            "v",
        ),
        (
            ActionSpec::new("Helm: Manifest", ReleaseManifest).hint("Manifest"),
            "m",
        ),
        (
            ActionSpec::new("Helm: History", ReleaseHistory).hint("History"),
            "h",
        ),
        (
            ActionSpec::new("Helm Releases: Upgrade…", Upgrade).hint("Upgrade…"),
            "u",
        ),
        (
            ActionSpec::new("Helm Releases: Roll Back…", RollBack).hint("Roll back…"),
            "b",
        ),
        (
            ActionSpec::new("Helm Releases: Uninstall…", Uninstall).hint("Uninstall…"),
            "ctrl-d",
        ),
        (
            ActionSpec::new("Helm Releases: Install Chart…", Install).hint("Install chart…"),
            "i",
        ),
        (
            ActionSpec::new("Helm Releases: Ask Agent", AskAgent).hint("Ask agent"),
            "shift-a",
        ),
        (
            ActionSpec::new("Helm: Copy helm Command…", HelmCommands).hint("Copy helm command"),
            "c",
        ),
    ] {
        ActionRegistry::register(cx, spec.bind(keys, Some(CONTEXT)));
    }
}

/// The objects of the selected release (from its manifest; nothing secret is kept).
pub(crate) enum HelmResources {
    Loading(#[allow(dead_code)] Task<()>),
    Ready(Vec<ManifestObject>),
    Failed(String),
}

pub fn release_key(row: &ReleaseRow) -> String {
    format!("helm:{}/{}/{}", row.namespace, row.name, row.driver.label())
}

pub fn status_tone(status: &str) -> Tone {
    match status {
        "deployed" => Tone::Good,
        "failed" => Tone::Bad,
        "superseded" | "uninstalled" => Tone::Muted,
        "uninstalling" => Tone::Warning,
        s if s.starts_with("pending") => Tone::Info,
        _ => Tone::Neutral,
    }
}

/// The kubeconfig file and context of a cluster, for `helm --kubeconfig --kube-context`.
pub fn kube_target(cluster: &ClusterId, cx: &App) -> Option<present::KubeTarget> {
    let manager = ConnectionManager::try_global(cx)?.read(cx);
    let info = manager.context(cluster)?;
    Some(present::KubeTarget {
        context: info.context.clone(),
        kubeconfig: info.file.clone(),
    })
}

/// The releases of `cluster`, if its watches run.
pub fn snapshot(cluster: &ClusterId, cx: &App) -> Option<Arc<Snapshot>> {
    Helm::global(cx)?.read(cx).snapshot(cluster, cx)
}

/// `5 releases in 3 namespaces · 1 failed`, for a toolbar.
pub fn summary(cluster: &ClusterId, cx: &App) -> String {
    let Some(snapshot) = snapshot(cluster, cx) else {
        return String::new();
    };
    let namespaces: std::collections::BTreeSet<&str> = snapshot
        .releases
        .iter()
        .map(|r| r.namespace.as_str())
        .collect();
    let failed = snapshot
        .releases
        .iter()
        .filter(|r| r.latest().status == "failed")
        .count();
    let mut text = format!(
        "{} releases in {} namespaces",
        snapshot.releases.len(),
        namespaces.len()
    );
    if failed > 0 {
        text.push_str(&format!(" · {failed} failed"));
    }
    text
}

/// The release list of one cluster with its details pane.
pub struct ReleasesView {
    cluster: ClusterId,
    /// Namespace the list is limited to (opened from a favorite).
    namespace: Option<String>,
    query: String,
    focus: FocusHandle,
    scroll: UniformListScrollHandle,
    selected: Option<String>,
    details_open: bool,
    /// The keys of the rows, in order (keyboard navigation).
    keys: Vec<String>,
    /// The rows the list shows (built on render, read by its `uniform_list`).
    rows: Vec<ReleaseRow>,
    resources: Option<(String, HelmResources)>,
    _lease: Option<HelmLease>,
    _subscriptions: Vec<Subscription>,
}

impl ReleasesView {
    pub fn new(cluster: ClusterId, cx: &mut Context<Self>) -> Self {
        let mut subscriptions = Vec::new();
        if let Some(helm) = Helm::global(cx) {
            subscriptions.push(cx.observe(&helm, |_, _, cx| cx.notify()));
        }
        if let Some(ops) = crate::ops::HelmOps::global(cx) {
            subscriptions.push(cx.observe(&ops, |_, _, cx| cx.notify()));
        }
        if let Some(manager) = ConnectionManager::try_global(cx) {
            subscriptions.push(cx.observe(&manager, |_, _, cx| cx.notify()));
        }
        let lease = Helm::watch(&cluster, cx);
        Self {
            cluster,
            namespace: None,
            query: String::new(),
            focus: cx.focus_handle(),
            scroll: UniformListScrollHandle::new(),
            selected: None,
            details_open: true,
            keys: Vec::new(),
            rows: Vec::new(),
            resources: None,
            _lease: lease,
            _subscriptions: subscriptions,
        }
    }

    pub fn cluster(&self) -> &ClusterId {
        &self.cluster
    }

    /// The filter (lowercase words, all must match).
    pub fn set_query(&mut self, query: String, cx: &mut Context<Self>) {
        if self.query != query {
            self.query = query;
            self.scroll.scroll_to_item(0, ScrollStrategy::Top);
            cx.notify();
        }
    }

    pub fn namespace(&self) -> Option<&str> {
        self.namespace.as_deref()
    }

    pub fn set_namespace(&mut self, namespace: Option<String>, cx: &mut Context<Self>) {
        self.namespace = namespace;
        cx.notify();
    }

    pub fn selected_key(&self) -> Option<&String> {
        self.selected.as_ref()
    }

    pub fn select(&mut self, key: String, cx: &mut Context<Self>) {
        if let Some(index) = self.keys.iter().position(|k| k == &key) {
            self.scroll.scroll_to_item(index, ScrollStrategy::Nearest);
        }
        self.selected = Some(key);
        self.details_open = true;
        cx.notify();
    }

    pub fn move_selection(&mut self, delta: isize, cx: &mut Context<Self>) {
        if self.keys.is_empty() {
            return;
        }
        let current = self
            .selected
            .as_ref()
            .and_then(|k| self.keys.iter().position(|x| x == k));
        let next = match current {
            None => 0,
            Some(i) => (i as isize + delta).clamp(0, self.keys.len() as isize - 1) as usize,
        };
        let key = self.keys[next].clone();
        if let Some(index) = self.keys.iter().position(|k| k == &key) {
            self.scroll.scroll_to_item(index, ScrollStrategy::Nearest);
        }
        self.selected = Some(key);
        cx.notify();
    }

    pub fn toggle_details(&mut self, cx: &mut Context<Self>) {
        self.details_open = !self.details_open || self.selected.is_none();
        if self.selected.is_none()
            && let Some(first) = self.keys.first().cloned()
        {
            self.selected = Some(first);
        }
        cx.notify();
    }

    fn matches(query: &str, haystack: &str) -> bool {
        let haystack = haystack.to_lowercase();
        query.split_whitespace().all(|w| haystack.contains(w))
    }

    pub fn selected_release(&self, cx: &App) -> Option<ReleaseRow> {
        let key = self.selected.as_ref()?;
        snapshot(&self.cluster, cx)?
            .releases
            .iter()
            .find(|r| &release_key(r) == key)
            .cloned()
    }

    /// The rows after the namespace and the filter, sorted by name.
    fn build_rows(&mut self, snapshot: &Snapshot) {
        let query = self.query.trim().to_lowercase();
        let namespace = self.namespace.clone();
        self.rows = snapshot
            .releases
            .iter()
            .filter(|r| namespace.as_ref().is_none_or(|ns| &r.namespace == ns))
            .filter(|r| {
                query.is_empty() || {
                    let chart = r.summary().map(|s| s.chart_label()).unwrap_or_default();
                    Self::matches(
                        &query,
                        &format!("{} {} {} {}", r.name, r.namespace, chart, r.latest().status),
                    )
                }
            })
            .cloned()
            .collect();
        self.rows
            .sort_by(|a, b| (&a.name, &a.namespace).cmp(&(&b.name, &b.namespace)));
        self.keys = self.rows.iter().map(release_key).collect();
        if self.selected.is_none()
            && let Some(first) = self.keys.first().cloned()
        {
            self.selected = Some(first);
        }
    }

    fn columns(&self) -> Vec<ColumnDef> {
        let details = self.details_open && self.selected.is_some();
        let mut columns = vec![
            ColumnDef::new(
                "name",
                "Name",
                ColumnWidth::Flex {
                    weight: 1.1,
                    min: 150.0,
                },
            ),
            ColumnDef::new("namespace", "Namespace", ColumnWidth::Fixed(116.0)),
            ColumnDef::new(
                "chart",
                "Chart",
                ColumnWidth::Flex {
                    weight: 1.2,
                    min: 150.0,
                },
            ),
        ];
        if !details {
            columns.push(ColumnDef::new(
                "app",
                "App version",
                ColumnWidth::Fixed(96.0),
            ));
        }
        columns.extend([
            ColumnDef::new("revision", "Revision", ColumnWidth::Fixed(76.0)),
            ColumnDef::new("status", "Status", ColumnWidth::Fixed(140.0)),
            ColumnDef::new("updated", "Updated", ColumnWidth::Fixed(70.0)),
        ]);
        columns
    }

    fn render_problem(
        &self,
        problem: String,
        scope: Option<String>,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let colors = cx.colors().clone();
        let namespaces = ConnectionManager::try_global(cx)
            .map(|m| m.read(cx).namespaces(&self.cluster).names)
            .unwrap_or_default();
        let cluster = self.cluster.clone();
        let current = scope.clone();
        let menu = MenuButton::new("helm-scope")
            .ghost()
            .compact()
            .child(
                h_flex()
                    .gap(u(4.0))
                    .text_size(u(12.0))
                    .child(format!(
                        "Namespace: {}",
                        scope.unwrap_or_else(|| "all".into())
                    ))
                    .child(Icon::new(IconName::ChevronDown).size(11.0)),
            )
            .dropdown_menu(move |mut menu, _, _| {
                menu = menu.max_h(px(360.0)).scrollable(true);
                let pick = |value: Option<String>| {
                    let cluster = cluster.clone();
                    move |_: &gpui::ClickEvent, _: &mut Window, cx: &mut App| {
                        if let Some(helm) = Helm::global(cx) {
                            let value = value.clone();
                            helm.update(cx, |h, cx| h.set_scope(&cluster, value, cx));
                        }
                    }
                };
                menu = menu.item(
                    PopupMenuItem::new("All namespaces (needs list on secrets cluster-wide)")
                        .checked(current.is_none())
                        .on_click(pick(None)),
                );
                for ns in &namespaces {
                    menu = menu.item(
                        PopupMenuItem::new(ns.clone())
                            .checked(current.as_ref() == Some(ns))
                            .on_click(pick(Some(ns.clone()))),
                    );
                }
                menu
            });
        h_flex()
            .flex_none()
            .px(u(12.0))
            .py(u(6.0))
            .gap(u(8.0))
            .bg(colors.yellow.opacity(0.1))
            .border_b_1()
            .border_color(colors.yellow.opacity(0.3))
            .text_size(u(12.5))
            .child(Icon::new(IconName::Lock).size(13.0).color(colors.yellow))
            .child(div().flex_1().min_w_0().child(problem))
            .child(menu)
            .into_any_element()
    }

    fn render_rows(&mut self, range: Range<usize>, cx: &mut Context<Self>) -> Vec<AnyElement> {
        let colors = cx.colors().clone();
        let columns = self.columns();
        let selected = self.selected.clone();
        range
            .filter_map(|index| {
                let row = self.rows.get(index)?.clone();
                let key = release_key(&row);
                Some(
                    widgets::row(
                        ("helm-row", index),
                        selected.as_ref() == Some(&key),
                        ROW_HEIGHT,
                        &colors,
                    )
                    .children(columns.iter().map(|def| {
                        widgets::column_cell(def).child(cell(&row, def.id.as_ref(), &colors))
                    }))
                    .on_click(
                        cx.listener(move |this, event: &gpui::ClickEvent, window, cx| {
                            this.focus.focus(window, cx);
                            this.selected = Some(key.clone());
                            cx.notify();
                            if event.click_count() == 2 {
                                this.open_release(None, window, cx);
                            }
                        }),
                    )
                    .into_any_element(),
                )
            })
            .collect()
    }

    /// Loads the selected release's objects (its manifest, decoded on Tokio; only kinds and
    /// names are kept).
    fn ensure_resources(&mut self, row: &ReleaseRow, cx: &mut Context<Self>) {
        let key = format!("{}#{}", release_key(row), row.latest().revision);
        if self.resources.as_ref().is_some_and(|(k, _)| k == &key) {
            return;
        }
        let Some(client) =
            ConnectionManager::try_global(cx).and_then(|m| m.read(cx).client(&self.cluster))
        else {
            return;
        };
        let load = spawn_kube(cx, {
            let (driver, namespace, object) = (
                row.driver,
                row.namespace.clone(),
                row.latest().object.clone(),
            );
            async move {
                let release = crate::service::load(client, driver, namespace, object).await?;
                Ok::<_, String>(present::manifest_objects(&release.manifest))
            }
        });
        let task_key = key.clone();
        let task = cx.spawn(async move |this, cx| {
            let result = load.await;
            this.update(cx, |this, cx| {
                if this.resources.as_ref().is_some_and(|(k, _)| k == &task_key) {
                    this.resources = Some((
                        task_key,
                        match result {
                            Ok(objects) => HelmResources::Ready(objects),
                            Err(err) => HelmResources::Failed(err),
                        },
                    ));
                    cx.notify();
                }
            })
            .ok();
        });
        self.resources = Some((key, HelmResources::Loading(task)));
    }

    fn render_details(&mut self, row: ReleaseRow, cx: &mut Context<Self>) -> AnyElement {
        self.ensure_resources(&row, cx);
        let colors = cx.colors().clone();
        let latest = row.latest().clone();
        let mut summary = widgets::plain_section(&colors).child(
            h_flex()
                .gap(u(10.0))
                .child(widgets::pill(
                    latest.status.clone(),
                    status_tone(&latest.status),
                    &colors,
                ))
                .child(
                    div()
                        .text_size(u(12.0))
                        .text_color(colors.text_dim)
                        .child(format!(
                            "revision {} · {} ago",
                            latest.revision,
                            widgets::ago(latest.modified)
                        )),
                ),
        );
        match &row.summary {
            SummaryState::Ready(s) => {
                let mut rows = vec![
                    (
                        "Chart",
                        widgets::kv_mono(format!("{} {}", s.chart.name, s.chart.version)),
                    ),
                    (
                        "App version",
                        widgets::kv_mono(s.chart.app_version.clone().unwrap_or_else(|| "—".into())),
                    ),
                ];
                if let Some(first) = s.first_deployed {
                    rows.push((
                        "First deployed",
                        widgets::kv_text(format!("{} ago", widgets::ago(Some(first)))),
                    ));
                }
                if let Some(description) = &s.description {
                    rows.push(("Description", widgets::kv_text(description.clone())));
                }
                rows.push((
                    "Stored in",
                    widgets::kv_text(format!("{}s · {}", row.driver.label(), latest.object)),
                ));
                summary = summary.child(widgets::kv(rows, &colors));
            }
            SummaryState::Loading => {
                summary = summary.child(
                    div()
                        .text_size(u(12.0))
                        .text_color(colors.text_dim)
                        .child("Reading the release…"),
                );
            }
            SummaryState::Failed(err) => {
                summary = summary.child(widgets::note(
                    IconName::TriangleAlert,
                    colors.yellow,
                    err.clone(),
                    &colors,
                ));
            }
        }
        let writable = !crate::cli::read_only(&self.cluster, cx);
        summary = summary
            .child(
                h_flex()
                    .pt(u(4.0))
                    .gap(u(6.0))
                    .child(
                        Button::new("release-open")
                            .primary()
                            .icon(IconName::Anchor)
                            .label("Open release")
                            .on_click(cx.listener(|this, _, window, cx| {
                                this.open_release(None, window, cx)
                            })),
                    )
                    .when(writable, |this| {
                        this.child(
                            Button::new("release-upgrade")
                                .icon(IconName::ArrowUp)
                                .label("Upgrade…")
                                .on_click(
                                    cx.listener(|this, _, window, cx| this.upgrade(window, cx)),
                                ),
                        )
                    }),
            )
            .when(writable, |this| {
                this.child(
                    h_flex()
                        .gap(u(6.0))
                        .child(
                            Button::new("release-rollback")
                                .icon(IconName::RotateCcw)
                                .label("Roll back…")
                                .disabled(row.revisions.len() < 2)
                                .on_click(
                                    cx.listener(|this, _, window, cx| this.rollback(window, cx)),
                                ),
                        )
                        .child(
                            Button::new("release-uninstall")
                                .danger()
                                .icon(IconName::Trash)
                                .label("Uninstall…")
                                .on_click(
                                    cx.listener(|this, _, window, cx| this.uninstall(window, cx)),
                                ),
                        ),
                )
            })
            .child(
                h_flex()
                    .gap(u(6.0))
                    .child(
                        Button::new("release-ask")
                            .ghost()
                            .icon(IconName::Zap)
                            .label("Ask agent")
                            .on_click(
                                cx.listener(|this, _, window, cx| this.ask_agent(window, cx)),
                            ),
                    )
                    .child(
                        Button::new("release-commands")
                            .ghost()
                            .icon(IconName::Copy)
                            .label("Copy helm command…")
                            .on_click(cx.listener(|this, _, window, cx| this.commands(window, cx))),
                    ),
            );
        if let Some(op) = crate::ops::HelmOps::global(cx).and_then(|ops| {
            ops.read(cx)
                .running_on(&self.cluster, &row.namespace, &row.name)
                .map(|op| op.title())
        }) {
            summary = summary.child(widgets::note(
                IconName::RefreshCw,
                colors.accent,
                op,
                &colors,
            ));
        }
        if latest.status.starts_with("pending-")
            && crate::ops::HelmOps::global(cx).is_none_or(|ops| {
                ops.read(cx)
                    .running_on(&self.cluster, &row.namespace, &row.name)
                    .is_none()
            })
        {
            summary = summary.child(widgets::note(
                IconName::TriangleAlert,
                colors.yellow,
                stuck_hint(&row),
                &colors,
            ));
        }
        let mut history = widgets::section("History", &colors);
        for revision in row.revisions.iter().take(10) {
            history = history.child(
                h_flex()
                    .gap(u(8.0))
                    .py(u(2.0))
                    .text_size(u(12.0))
                    .child(
                        div()
                            .flex_none()
                            .w(u(28.0))
                            .font_family(fonts::MONO)
                            .child(revision.revision.to_string()),
                    )
                    .child(div().flex_1().child(widgets::pill(
                        revision.status.clone(),
                        status_tone(&revision.status),
                        &colors,
                    )))
                    .child(
                        div()
                            .flex_none()
                            .font_family(fonts::MONO)
                            .text_color(colors.text_dim)
                            .child(widgets::ago(revision.modified)),
                    ),
            );
        }
        if row.revisions.len() > 10 {
            history = history.child(
                div()
                    .text_size(u(11.5))
                    .text_color(colors.text_dim)
                    .child(format!("and {} older", row.revisions.len() - 10)),
            );
        }
        let resources = match self.resources.as_ref().map(|(_, r)| r) {
            Some(HelmResources::Ready(objects)) => {
                let mut own: Vec<&ManifestObject> = objects.iter().filter(|o| !o.hook).collect();
                own.sort_by_key(|o| (present::kind_rank(&o.kind), o.kind.clone(), o.name.clone()));
                let mut section = widgets::section(format!("Resources · {}", own.len()), &colors);
                let discovery = ConnectionManager::try_global(cx)
                    .and_then(|m| m.read(cx).discovery(&self.cluster));
                for (i, object) in own.iter().take(14).enumerate() {
                    let target = discovery.as_ref().and_then(|d| {
                        let (group, version) = object
                            .api_version
                            .split_once('/')
                            .unwrap_or(("", object.api_version.as_str()));
                        let info = d.by_gvk(&Gvk::new(group, version, &object.kind))?;
                        let namespace = info.namespaced.then(|| {
                            object
                                .namespace
                                .clone()
                                .unwrap_or_else(|| row.namespace.clone())
                        });
                        Some(ResourceRef::object(
                            self.cluster.clone(),
                            info.gvr.clone(),
                            namespace,
                            object.name.clone(),
                        ))
                    });
                    section = section.child(
                        h_flex()
                            .gap(u(8.0))
                            .text_size(u(12.0))
                            .child(
                                div()
                                    .flex_none()
                                    .w(u(96.0))
                                    .truncate()
                                    .text_color(colors.text_dim)
                                    .child(object.kind.clone()),
                            )
                            .child(match target {
                                Some(target) => widgets::link(
                                    ("release-object", i),
                                    object.name.clone(),
                                    &colors,
                                    move |_, window, cx| {
                                        let kind = ViewRegistry::object_view(cx, &target.gvr);
                                        window.dispatch_action(
                                            Box::new(OpenView(ViewRequest::for_resource(
                                                kind,
                                                target.clone(),
                                            ))),
                                            cx,
                                        );
                                    },
                                )
                                .flex_1()
                                .font_family(fonts::MONO)
                                .text_size(u(11.5))
                                .into_any_element(),
                                None => div()
                                    .flex_1()
                                    .truncate()
                                    .font_family(fonts::MONO)
                                    .text_size(u(11.5))
                                    .child(object.name.clone())
                                    .into_any_element(),
                            }),
                    );
                }
                if own.len() > 14 {
                    section = section.child(
                        div()
                            .text_size(u(11.5))
                            .text_color(colors.text_dim)
                            .child(format!("and {} more", own.len() - 14)),
                    );
                }
                section
            }
            Some(HelmResources::Failed(err)) => widgets::section("Resources", &colors).child(
                widgets::note(IconName::TriangleAlert, colors.yellow, err.clone(), &colors),
            ),
            _ => widgets::section("Resources", &colors).child(
                div()
                    .text_size(u(12.0))
                    .text_color(colors.text_dim)
                    .child("Reading the manifest…"),
            ),
        };
        v_flex()
            .flex_none()
            .w(u(350.0))
            .h_full()
            .bg(colors.panel)
            .border_l_1()
            .border_color(colors.border)
            .child(
                h_flex()
                    .flex_none()
                    .h(u(36.0))
                    .px(u(12.0))
                    .gap(u(8.0))
                    .border_b_1()
                    .border_color(colors.border_variant)
                    .child(Icon::new(IconName::Anchor).size(14.0).color(colors.accent))
                    .child(
                        div()
                            .flex_1()
                            .truncate()
                            .font_family(fonts::MONO)
                            .font_weight(FontWeight::MEDIUM)
                            .child(row.name.clone()),
                    )
                    .child(
                        div()
                            .text_size(u(11.5))
                            .text_color(colors.text_dim)
                            .child(row.namespace.clone()),
                    )
                    .child(
                        kubyl_ui::IconButton::new("release-details-close", IconName::X)
                            .icon_size(13.0)
                            .on_click(cx.listener(|this, _, _, cx| {
                                this.details_open = false;
                                cx.notify();
                            })),
                    ),
            )
            .child(
                v_flex()
                    .id("release-details")
                    .flex_1()
                    .min_h_0()
                    .overflow_y_scroll()
                    .child(summary)
                    .child(history)
                    .child(resources),
            )
            .into_any_element()
    }

    pub fn open_release(
        &mut self,
        tab: Option<ReleaseTab>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let Some(row) = self.selected_release(cx) else {
            return;
        };
        crate::release::open(&self.cluster, &row, tab, window, cx);
    }

    pub fn upgrade(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if let Some(row) = self.selected_release(cx) {
            crate::dialogs::open_upgrade(self.cluster.clone(), row, window, cx);
        }
    }

    pub fn rollback(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if let Some(row) = self.selected_release(cx) {
            crate::dialogs::open_rollback(self.cluster.clone(), row, None, window, cx);
        }
    }

    pub fn uninstall(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if let Some(row) = self.selected_release(cx) {
            crate::dialogs::open_uninstall(self.cluster.clone(), row, window, cx);
        }
    }

    pub fn install(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        crate::dialogs::open_install(
            crate::dialogs::InstallRequest {
                cluster: Some(self.cluster.clone()),
                namespace: self.namespace.clone(),
                ..Default::default()
            },
            window,
            cx,
        );
    }

    pub fn ask_agent(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if let Some(row) = self.selected_release(cx) {
            crate::release::ask_agent(&self.cluster, &row, window, cx);
        }
    }

    pub fn commands(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Some(row) = self.selected_release(cx) else {
            return;
        };
        let latest = row.latest().revision;
        let commands = present::commands(
            &row.name,
            &row.namespace,
            latest,
            latest,
            row.driver,
            kube_target(&self.cluster, cx).as_ref(),
        );
        crate::dialogs::open_helm_commands(
            format!("helm commands for {}", row.name),
            commands,
            window,
            cx,
        );
    }
}

impl Focusable for ReleasesView {
    fn focus_handle(&self, _: &App) -> FocusHandle {
        self.focus.clone()
    }
}

impl Render for ReleasesView {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let colors = cx.colors().clone();
        let focus_area = div()
            .key_context(CONTEXT)
            .track_focus(&self.focus)
            .flex()
            .flex_1()
            .min_h_0()
            .min_w_0()
            .on_action(
                cx.listener(|this, _: &OpenRelease, window, cx| {
                    this.open_release(None, window, cx)
                }),
            )
            .on_action(cx.listener(|this, _: &ReleaseValues, window, cx| {
                this.open_release(Some(ReleaseTab::Values), window, cx)
            }))
            .on_action(cx.listener(|this, _: &ReleaseManifest, window, cx| {
                this.open_release(Some(ReleaseTab::Manifest), window, cx)
            }))
            .on_action(cx.listener(|this, _: &ReleaseHistory, window, cx| {
                this.open_release(Some(ReleaseTab::History), window, cx)
            }))
            .on_action(cx.listener(|this, _: &HelmCommands, window, cx| this.commands(window, cx)))
            .on_action(cx.listener(|this, _: &Upgrade, window, cx| this.upgrade(window, cx)))
            .on_action(
                cx.listener(|this, _: &crate::UpgradeRelease, window, cx| this.upgrade(window, cx)),
            )
            .on_action(cx.listener(|this, _: &RollBack, window, cx| this.rollback(window, cx)))
            .on_action(
                cx.listener(|this, _: &crate::RollBackRelease, window, cx| {
                    this.rollback(window, cx)
                }),
            )
            .on_action(cx.listener(|this, _: &Uninstall, window, cx| this.uninstall(window, cx)))
            .on_action(
                cx.listener(|this, _: &crate::UninstallRelease, window, cx| {
                    this.uninstall(window, cx)
                }),
            )
            .on_action(cx.listener(|this, _: &Install, window, cx| this.install(window, cx)))
            .on_action(
                cx.listener(|this, _: &crate::InstallChart, window, cx| this.install(window, cx)),
            )
            .on_action(cx.listener(|this, _: &AskAgent, window, cx| this.ask_agent(window, cx)));
        let Some(snapshot) = snapshot(&self.cluster, cx) else {
            self.keys.clear();
            return v_flex()
                .size_full()
                .child(focus_area.child(widgets::empty("Connecting to the cluster…", &colors)));
        };
        self.build_rows(&snapshot);
        let namespace = self.namespace.clone();
        let mut banners: Vec<AnyElement> = Vec::new();
        if let Some(problem) = &snapshot.problem {
            banners.push(self.render_problem(problem.clone(), snapshot.scope.clone(), cx));
        }
        if let Some(ns) = &namespace {
            banners.push(
                h_flex()
                    .flex_none()
                    .h(u(34.0))
                    .px(u(12.0))
                    .gap(u(8.0))
                    .border_b_1()
                    .border_color(colors.border_variant)
                    .text_size(u(12.0))
                    .text_color(colors.text_muted)
                    .child("Namespace")
                    .child(
                        div()
                            .id("helm-namespace-chip")
                            .cursor_pointer()
                            .child(Chip::new(ns.clone()).mono().selected(true).removable())
                            .on_click(cx.listener(|this, _, _, cx| {
                                this.namespace = None;
                                cx.notify();
                            })),
                    )
                    .into_any_element(),
            );
        }
        let body = if self.rows.is_empty() {
            widgets::empty(
                if snapshot.loading {
                    "Loading Helm releases…"
                } else if snapshot.releases.is_empty() {
                    if snapshot.problem.is_some() {
                        "No Helm releases Kubyl can read."
                    } else {
                        "No Helm releases (Kubyl reads Helm 3 releases stored in Secrets or ConfigMaps)."
                    }
                } else {
                    "No releases match the filter."
                },
                &colors,
            )
        } else {
            let columns = self.columns();
            v_flex()
                .flex_1()
                .min_h_0()
                .child(widgets::header(&columns, &colors))
                .child(
                    uniform_list(
                        "helm-rows",
                        self.rows.len(),
                        cx.processor(|this, range: Range<usize>, _, cx| {
                            this.render_rows(range, cx)
                        }),
                    )
                    .flex_1()
                    .track_scroll(&self.scroll),
                )
                .into_any_element()
        };
        let details = self
            .details_open
            .then(|| self.selected_release(cx))
            .flatten()
            .map(|row| self.render_details(row, cx));
        v_flex().size_full().children(banners).child(
            focus_area
                .items_start()
                .child(v_flex().flex_1().min_w_0().h_full().child(body))
                .children(details),
        )
    }
}

/// What to do about a release left `pending-*` (an interrupted install, upgrade or rollback).
pub fn stuck_hint(row: &ReleaseRow) -> String {
    let status = &row.latest().status;
    let deployed = row
        .revisions
        .iter()
        .skip(1)
        .find(|r| matches!(r.status.as_str(), "deployed" | "superseded"));
    match deployed {
        Some(revision) => format!(
            "The release is {status}: no Kubyl operation runs on it. If no helm runs elsewhere, an operation was interrupted and Helm refuses new ones until you roll back to revision {} (the last deployed one).",
            revision.revision
        ),
        None => format!(
            "The release is {status} and never deployed: if no helm runs elsewhere, uninstall it and install again."
        ),
    }
}

/// Makes a [`ReleasesView`] for `cluster`.
pub fn new_view(cluster: ClusterId, cx: &mut App) -> Entity<ReleasesView> {
    cx.new(|cx| ReleasesView::new(cluster, cx))
}

fn cell(row: &ReleaseRow, column: &str, colors: &Colors) -> AnyElement {
    let summary = row.summary();
    let dim = |text: String| {
        div()
            .truncate()
            .font_family(fonts::MONO)
            .text_size(u(12.0))
            .text_color(colors.text_muted)
            .child(text)
            .into_any_element()
    };
    match column {
        "name" => widgets::mono(row.name.clone()),
        "namespace" => dim(row.namespace.clone()),
        "chart" => match (&row.summary, summary) {
            (_, Some(s)) => widgets::mono(s.chart_label()),
            (SummaryState::Failed(_), _) => dim("unreadable".into()),
            _ => dim("…".into()),
        },
        "app" => dim(summary
            .and_then(|s| s.chart.app_version.clone())
            .unwrap_or_default()),
        "revision" => div()
            .w_full()
            .flex()
            .justify_end()
            .pr(u(16.0))
            .font_family(fonts::MONO)
            .text_size(u(12.0))
            .child(row.latest().revision.to_string())
            .into_any_element(),
        "status" => widgets::pill(
            row.latest().status.clone(),
            status_tone(&row.latest().status),
            colors,
        ),
        "updated" => dim(widgets::ago(row.latest().modified)),
        _ => div().into_any_element(),
    }
}
