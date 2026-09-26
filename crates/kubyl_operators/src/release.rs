//! A Helm release's tab (board 7): Values (user-supplied or with the chart's defaults, masked
//! until revealed), Manifest (Secret data masked), Notes, History and Resources, for any
//! revision. Read-only: rollback and uninstall are commands to copy.
//!
//! The decoded release lives only in this tab; revealing values is per tab and never
//! remembered; copying them is an explicit action.

use std::collections::HashMap;
use std::ops::Range;
use std::sync::Arc;

use gpui::{
    AnyElement, App, AppContext as _, Context, FocusHandle, Focusable, FontWeight, Global,
    IntoElement, Render, SharedString, Task, UniformListScrollHandle, Window, actions, div,
    prelude::*, px, uniform_list,
};
use gpui_component::button::Button as MenuButton;
use gpui_component::menu::{DropdownMenu as _, PopupMenuItem};
use kubyl_core::actions::OpenView;
use kubyl_core::{
    ActionRegistry, ActionSpec, ActiveContext, ClusterId, Gvk, Notification, NotificationCenter,
    ResourceRef, TabView, ViewKind, ViewRegistry, ViewRequest, spawn_kube,
};
use kubyl_kube::ConnectionManager;
use kubyl_ui::{ActiveColors, Button, Colors, Icon, IconName, fonts, h_flex, sizes, u, v_flex};

use crate::helm::decode::{Driver, Release, Summary};
use crate::helm::present::{self, Line, ManifestObject};
use crate::helm::service::{self, Helm, HelmLease, ReleaseRow, Revision};
use crate::widgets;

/// `ViewKind::Custom` of a release tab; the target is the storage object of a revision.
pub const VIEW_KIND: &str = "helm_release";
pub const CONTEXT: &str = "HelmRelease";

actions!(
    helm_release,
    [
        /// Values tab.
        ShowValues,
        /// Manifest tab.
        ShowManifest,
        /// Notes tab.
        ShowNotes,
        /// History tab.
        ShowHistory,
        /// Resources tab.
        ShowResources,
        /// Reveals or masks the values.
        ToggleReveal,
        /// The previous (older) revision.
        OlderRevision,
        /// The next (newer) revision.
        NewerRevision,
        /// Copies a `helm` command.
        CopyHelmCommand,
    ]
);

/// The tabs of a release.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum ReleaseTab {
    #[default]
    Values,
    Manifest,
    Notes,
    History,
    Resources,
}

#[derive(Default)]
struct PendingTabs(HashMap<ResourceRef, ReleaseTab>);

impl Global for PendingTabs {}

/// Opens a release's tab (on `tab`).
pub fn open(
    cluster: &ClusterId,
    row: &ReleaseRow,
    tab: Option<ReleaseTab>,
    window: &mut Window,
    cx: &mut App,
) {
    let target = row.object_ref(cluster);
    if let Some(tab) = tab {
        cx.default_global::<PendingTabs>()
            .0
            .insert(target.clone(), tab);
    }
    window.dispatch_action(
        Box::new(OpenView(ViewRequest::for_resource(
            ViewKind::Custom(VIEW_KIND.into()),
            target,
        ))),
        cx,
    );
    cx.refresh_windows();
}

pub(crate) fn init(cx: &mut App) {
    ViewRegistry::register(
        cx,
        ViewKind::Custom(VIEW_KIND.into()),
        |request, window, cx| {
            let target = request.target.clone().filter(ResourceRef::is_object)?;
            Some(Box::new(cx.new(|cx| ReleaseView::new(target, window, cx))))
        },
    );
    for (spec, keys) in [
        (
            ActionSpec::new("Helm Release: Values", ShowValues).hint("Values"),
            "1",
        ),
        (
            ActionSpec::new("Helm Release: Manifest", ShowManifest).hint("Manifest"),
            "2",
        ),
        (
            ActionSpec::new("Helm Release: Notes", ShowNotes).hint("Notes"),
            "3",
        ),
        (
            ActionSpec::new("Helm Release: History", ShowHistory).hint("History"),
            "4",
        ),
        (
            ActionSpec::new("Helm Release: Resources", ShowResources).hint("Resources"),
            "5",
        ),
        (
            ActionSpec::new("Helm Release: Reveal or Mask Values", ToggleReveal)
                .hint("Reveal/mask"),
            "r",
        ),
        (
            ActionSpec::new("Helm Release: Older Revision", OlderRevision).hint("Older"),
            "[",
        ),
        (
            ActionSpec::new("Helm Release: Newer Revision", NewerRevision).hint("Newer"),
            "]",
        ),
        (
            ActionSpec::new("Helm Release: Copy helm Command…", CopyHelmCommand)
                .hint("Copy helm command"),
            "c",
        ),
    ] {
        ActionRegistry::register(cx, spec.bind(keys, Some(CONTEXT)));
    }
}

/// A loaded revision, prepared for the tabs (lines built once, off the UI thread).
struct Loaded {
    release: Release,
    /// Values lines: (all values?, revealed?) → lines.
    values: HashMap<(bool, bool), Arc<Vec<Line>>>,
    manifest: Arc<Vec<Line>>,
    notes: Arc<Vec<Line>>,
    objects: Vec<ManifestObject>,
}

enum State {
    Loading(#[allow(dead_code)] Task<()>),
    Ready(Box<Loaded>),
    Failed(String),
}

pub struct ReleaseView {
    cluster: ClusterId,
    driver: Driver,
    namespace: String,
    /// The storage object of the shown revision.
    object: String,
    /// The release's name (from the storage object's labels once known).
    name: String,
    tab: ReleaseTab,
    all_values: bool,
    reveal: bool,
    state: State,
    history: Option<HashMap<String, Result<Summary, String>>>,
    _history_task: Option<Task<()>>,
    focus: FocusHandle,
    scroll: UniformListScrollHandle,
    _lease: Option<HelmLease>,
    _subscriptions: Vec<gpui::Subscription>,
}

impl ReleaseView {
    pub fn new(target: ResourceRef, window: &mut Window, cx: &mut Context<Self>) -> Self {
        let _ = window;
        let driver = if target.gvr.resource == "configmaps" {
            Driver::ConfigMap
        } else {
            Driver::Secret
        };
        let object = target.name.clone().unwrap_or_default();
        // `sh.helm.release.v1.<name>.v<revision>`.
        let name = object
            .strip_prefix("sh.helm.release.v1.")
            .and_then(|rest| rest.rsplit_once(".v").map(|(n, _)| n.to_string()))
            .unwrap_or_else(|| object.clone());
        let mut subscriptions = Vec::new();
        if let Some(helm) = Helm::global(cx) {
            subscriptions.push(cx.observe(&helm, |_, _, cx| cx.notify()));
        }
        let lease = Helm::watch(&target.cluster, cx);
        let tab = cx
            .try_global::<PendingTabs>()
            .and_then(|p| p.0.get(&target).copied());
        if tab.is_some() {
            cx.default_global::<PendingTabs>().0.remove(&target);
        }
        let mut this = Self {
            cluster: target.cluster.clone(),
            driver,
            namespace: target.namespace.clone().unwrap_or_default(),
            object,
            name,
            tab: tab.unwrap_or_default(),
            all_values: false,
            reveal: false,
            state: State::Failed(String::new()),
            history: None,
            _history_task: None,
            focus: cx.focus_handle(),
            scroll: UniformListScrollHandle::new(),
            _lease: lease,
            _subscriptions: subscriptions,
        };
        this.load(cx);
        this
    }

    fn target(&self) -> ResourceRef {
        service::object_ref(&self.cluster, self.driver, &self.namespace, &self.object)
    }

    /// Decodes the shown revision on Tokio and prepares its lines there too.
    fn load(&mut self, cx: &mut Context<Self>) {
        let Some(client) =
            ConnectionManager::try_global(cx).and_then(|m| m.read(cx).client(&self.cluster))
        else {
            self.state = State::Failed(format!(
                "Not connected to {}.",
                ConnectionManager::try_global(cx)
                    .map(|m| m.read(cx).display_name(&self.cluster).to_string())
                    .unwrap_or_default()
            ));
            return;
        };
        let (driver, namespace, object) =
            (self.driver, self.namespace.clone(), self.object.clone());
        let work = spawn_kube(cx, async move {
            let release = service::load(client, driver, namespace, object).await?;
            let mut values = HashMap::new();
            // Masked lines are ready at once; revealed ones are built when asked for.
            values.insert(
                (false, false),
                Arc::new(present::value_lines(&release.values, true)),
            );
            values.insert(
                (true, false),
                Arc::new(present::value_lines(&release.all_values(), true)),
            );
            let manifest_text = present::mask_secrets(&release.manifest);
            let manifest = Arc::new(present::text_lines(&manifest_text));
            let notes = Arc::new(
                release
                    .notes
                    .as_deref()
                    .map(|n| {
                        n.lines()
                            .map(|l| {
                                vec![present::Span {
                                    text: l.to_string(),
                                    tone: present::Tone::Plain,
                                }]
                            })
                            .collect()
                    })
                    .unwrap_or_default(),
            );
            let objects = present::manifest_objects(&release.manifest);
            Ok::<_, String>(Box::new(Loaded {
                release,
                values,
                manifest,
                notes,
                objects,
            }))
        });
        let task = cx.spawn(async move |this, cx| {
            let result = work.await;
            this.update(cx, |this, cx| {
                this.state = match result {
                    Ok(loaded) => {
                        this.name = loaded.release.summary.name.clone();
                        State::Ready(loaded)
                    }
                    Err(err) => State::Failed(err),
                };
                cx.notify();
            })
            .ok();
        });
        self.state = State::Loading(task);
    }

    fn row(&self, cx: &App) -> Option<ReleaseRow> {
        let snapshot = Helm::global(cx)?.read(cx).snapshot(&self.cluster, cx)?;
        snapshot
            .releases
            .iter()
            .find(|r| {
                r.namespace == self.namespace && r.name == self.name && r.driver == self.driver
            })
            .cloned()
    }

    fn revisions(&self, cx: &App) -> Vec<Revision> {
        self.row(cx).map(|r| r.revisions).unwrap_or_default()
    }

    fn revision(&self) -> Option<u32> {
        self.object
            .rsplit_once(".v")
            .and_then(|(_, r)| r.parse().ok())
    }

    fn show_revision(&mut self, object: String, cx: &mut Context<Self>) {
        if object == self.object {
            return;
        }
        self.object = object;
        self.reveal = false;
        self.scroll = UniformListScrollHandle::new();
        self.load(cx);
        cx.notify();
    }

    fn step_revision(&mut self, older: bool, cx: &mut Context<Self>) {
        let revisions = self.revisions(cx);
        let Some(index) = revisions.iter().position(|r| r.object == self.object) else {
            return;
        };
        let next = if older {
            index + 1
        } else {
            index.wrapping_sub(1)
        };
        if let Some(revision) = revisions.get(next) {
            self.show_revision(revision.object.clone(), cx);
        }
    }

    fn set_tab(&mut self, tab: ReleaseTab, cx: &mut Context<Self>) {
        self.tab = tab;
        self.scroll = UniformListScrollHandle::new();
        if tab == ReleaseTab::History {
            self.load_history(cx);
        }
        cx.notify();
    }

    fn toggle_reveal(&mut self, cx: &mut Context<Self>) {
        self.reveal = !self.reveal;
        if let State::Ready(loaded) = &mut self.state
            && self.reveal
        {
            for all in [false, true] {
                loaded.values.entry((all, true)).or_insert_with(|| {
                    let values = if all {
                        loaded.release.all_values()
                    } else {
                        loaded.release.values.clone()
                    };
                    Arc::new(present::value_lines(&values, false))
                });
            }
        }
        cx.notify();
    }

    /// Copies the values as YAML: an explicit action, so it copies what the user sees
    /// (revealed or not) and says the values may hold passwords.
    fn copy_values(&mut self, cx: &mut Context<Self>) {
        let State::Ready(loaded) = &self.state else {
            return;
        };
        let values = if self.all_values {
            loaded.release.all_values()
        } else {
            loaded.release.values.clone()
        };
        let text = if self.reveal {
            kubyl_yaml::render::to_yaml(&kubyl_yaml::render::sorted(values))
        } else {
            present::value_lines(&values, true)
                .iter()
                .map(|l| l.iter().map(|s| s.text.as_str()).collect::<String>())
                .collect::<Vec<_>>()
                .join("\n")
        };
        cx.write_to_clipboard(gpui::ClipboardItem::new_string(text));
        NotificationCenter::push(
            cx,
            Notification::info(if self.reveal {
                "Copied the values. They may hold passwords: paste them with care."
            } else {
                "Copied the values with strings and numbers masked."
            }),
        );
    }

    fn load_history(&mut self, cx: &mut Context<Self>) {
        if self.history.is_some() {
            return;
        }
        let Some(client) =
            ConnectionManager::try_global(cx).and_then(|m| m.read(cx).client(&self.cluster))
        else {
            return;
        };
        let objects: Vec<String> = self
            .revisions(cx)
            .into_iter()
            .take(20)
            .map(|r| r.object)
            .collect();
        let work = spawn_kube(
            cx,
            service::load_summaries(client, self.driver, self.namespace.clone(), objects),
        );
        self.history = Some(HashMap::new());
        self._history_task = Some(cx.spawn(async move |this, cx| {
            let results = work.await;
            this.update(cx, |this, cx| {
                this.history = Some(results.into_iter().collect());
                cx.notify();
            })
            .ok();
        }));
    }

    fn commands(&self, cx: &App) -> Vec<present::Command> {
        let latest = self.revisions(cx).first().map(|r| r.revision).unwrap_or(1);
        present::commands(
            &self.name,
            &self.namespace,
            self.revision().unwrap_or(latest),
            latest,
            crate::view::helm_context(&self.cluster, cx).as_deref(),
        )
    }

    fn copy_command(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let commands = self.commands(cx);
        crate::dialogs::open_helm_commands(
            format!("helm commands for {}", self.name),
            commands,
            window,
            cx,
        );
    }

    fn lines(&self) -> Option<Arc<Vec<Line>>> {
        let State::Ready(loaded) = &self.state else {
            return None;
        };
        match self.tab {
            ReleaseTab::Values => loaded
                .values
                .get(&(self.all_values, self.reveal))
                .or_else(|| loaded.values.get(&(self.all_values, false)))
                .cloned(),
            ReleaseTab::Manifest => Some(loaded.manifest.clone()),
            ReleaseTab::Notes => Some(loaded.notes.clone()),
            _ => None,
        }
    }

    fn render_lines(&mut self, lines: Arc<Vec<Line>>, cx: &mut Context<Self>) -> AnyElement {
        let colors = cx.colors().clone();
        if lines.is_empty() {
            return widgets::empty(
                match self.tab {
                    ReleaseTab::Notes => "The chart has no notes.",
                    ReleaseTab::Values if !self.all_values => {
                        "No user-supplied values (the chart's defaults apply)."
                    }
                    _ => "Empty.",
                },
                &colors,
            );
        }
        let count = lines.len();
        uniform_list(
            "release-lines",
            count,
            cx.processor(move |this, range: Range<usize>, _, cx| {
                let colors = cx.colors().clone();
                let lines = this.lines().unwrap_or_default();
                range
                    .filter_map(|i| {
                        lines
                            .get(i)
                            .map(|line| widgets::yaml_line(i + 1, line, &colors))
                    })
                    .collect::<Vec<_>>()
            }),
        )
        .size_full()
        .pt(u(6.0))
        .track_scroll(&self.scroll)
        .into_any_element()
    }

    fn render_history(&mut self, cx: &mut Context<Self>) -> AnyElement {
        let colors = cx.colors().clone();
        let revisions = self.revisions(cx);
        let history = self.history.clone().unwrap_or_default();
        let current = self.object.clone();
        let rows: Vec<AnyElement> = revisions
            .into_iter()
            .enumerate()
            .map(|(i, revision)| {
                let summary = history.get(&revision.object);
                let (chart, app, description) = match summary {
                    Some(Ok(s)) => (
                        s.chart_label(),
                        s.chart.app_version.clone().unwrap_or_default(),
                        s.description.clone().unwrap_or_default(),
                    ),
                    Some(Err(err)) => (String::new(), String::new(), err.clone()),
                    None => ("…".into(), String::new(), String::new()),
                };
                let object = revision.object.clone();
                widgets::row(
                    ("history-row", i),
                    revision.object == current,
                    32.0,
                    &colors,
                )
                .gap(u(8.0))
                .child(
                    div()
                        .flex_none()
                        .w(u(70.0))
                        .font_family(fonts::MONO)
                        .child(revision.revision.to_string()),
                )
                .child(div().flex_none().w(u(130.0)).child(widgets::pill(
                    revision.status.clone(),
                    crate::view::helm_status_tone(&revision.status),
                    &colors,
                )))
                .child(
                    div()
                        .flex_1()
                        .min_w_0()
                        .truncate()
                        .font_family(fonts::MONO)
                        .text_size(u(12.0))
                        .child(chart),
                )
                .child(
                    div()
                        .flex_none()
                        .w(u(100.0))
                        .truncate()
                        .font_family(fonts::MONO)
                        .text_size(u(12.0))
                        .text_color(colors.text_muted)
                        .child(app),
                )
                .child(
                    div()
                        .flex_1()
                        .min_w_0()
                        .truncate()
                        .text_color(colors.text_muted)
                        .child(description),
                )
                .child(
                    div()
                        .flex_none()
                        .w(u(70.0))
                        .font_family(fonts::MONO)
                        .text_size(u(12.0))
                        .text_color(colors.text_muted)
                        .child(widgets::ago(revision.modified)),
                )
                .on_click(cx.listener(move |this, _, _, cx| {
                    this.show_revision(object.clone(), cx);
                    this.set_tab(ReleaseTab::Values, cx);
                }))
                .into_any_element()
            })
            .collect();
        v_flex()
            .id("release-history")
            .size_full()
            .overflow_y_scroll()
            .child(
                h_flex()
                    .flex_none()
                    .h(u(sizes::TABLE_HEADER))
                    .px(u(12.0))
                    .gap(u(8.0))
                    .bg(colors.subheader_background)
                    .border_b_1()
                    .border_color(colors.border_variant)
                    .text_size(u(11.5))
                    .text_color(colors.text_dim)
                    .child(div().w(u(70.0)).child("REVISION"))
                    .child(div().w(u(130.0)).child("STATUS"))
                    .child(div().flex_1().child("CHART"))
                    .child(div().w(u(100.0)).child("APP VERSION"))
                    .child(div().flex_1().child("DESCRIPTION"))
                    .child(div().w(u(70.0)).child("UPDATED")),
            )
            .children(rows)
            .into_any_element()
    }

    fn render_resources(&mut self, cx: &mut Context<Self>) -> AnyElement {
        let colors = cx.colors().clone();
        let State::Ready(loaded) = &self.state else {
            return widgets::empty("Loading…", &colors);
        };
        let discovery =
            ConnectionManager::try_global(cx).and_then(|m| m.read(cx).discovery(&self.cluster));
        let rows: Vec<AnyElement> = loaded
            .objects
            .iter()
            .enumerate()
            .map(|(i, object)| {
                let (group, version) = object
                    .api_version
                    .split_once('/')
                    .unwrap_or(("", object.api_version.as_str()));
                let info = discovery
                    .as_ref()
                    .and_then(|d| d.by_gvk(&Gvk::new(group, version, &object.kind)).cloned());
                let namespace = match &info {
                    Some(info) if info.namespaced => Some(
                        object
                            .namespace
                            .clone()
                            .unwrap_or_else(|| self.namespace.clone()),
                    ),
                    Some(_) => None,
                    None => object.namespace.clone(),
                };
                let target = info.map(|info| {
                    ResourceRef::object(
                        self.cluster.clone(),
                        info.gvr.clone(),
                        namespace.clone(),
                        object.name.clone(),
                    )
                });
                let name: AnyElement = match target {
                    Some(target) => widgets::link(
                        ("release-resource", i),
                        object.name.clone(),
                        &colors,
                        move |_, window, cx| {
                            let kind = ViewRegistry::object_view(cx, &target.gvr);
                            window.dispatch_action(
                                Box::new(OpenView(ViewRequest::for_resource(kind, target.clone()))),
                                cx,
                            );
                        },
                    )
                    .font_family(fonts::MONO)
                    .text_size(u(12.0))
                    .into_any_element(),
                    None => widgets::mono(object.name.clone()),
                };
                widgets::row(("resource-row", i), false, 30.0, &colors)
                    .gap(u(8.0))
                    .child(
                        div()
                            .flex_none()
                            .w(u(200.0))
                            .truncate()
                            .text_color(colors.text_muted)
                            .child(object.kind.clone()),
                    )
                    .child(div().flex_1().min_w_0().child(name))
                    .child(
                        div()
                            .flex_none()
                            .w(u(160.0))
                            .truncate()
                            .font_family(fonts::MONO)
                            .text_size(u(12.0))
                            .text_color(colors.text_muted)
                            .child(namespace.unwrap_or_default()),
                    )
                    .child(
                        div()
                            .flex_none()
                            .w(u(60.0))
                            .text_size(u(11.5))
                            .text_color(colors.text_dim)
                            .child(if object.hook { "hook" } else { "" }),
                    )
                    .into_any_element()
            })
            .collect();
        v_flex()
            .id("release-resources")
            .size_full()
            .overflow_y_scroll()
            .children(rows)
            .into_any_element()
    }
}

impl Focusable for ReleaseView {
    fn focus_handle(&self, _: &App) -> FocusHandle {
        self.focus.clone()
    }
}

impl TabView for ReleaseView {
    fn tab_title(&self, _: &App) -> SharedString {
        self.name.clone().into()
    }

    fn tab_icon(&self, _: &App) -> Option<SharedString> {
        Some(IconName::Anchor.path())
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
        Some(ViewRequest::for_resource(
            ViewKind::Custom(VIEW_KIND.into()),
            self.target(),
        ))
    }
}

impl Render for ReleaseView {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let _ = window;
        let colors: Colors = cx.colors().clone();
        let summary = match &self.state {
            State::Ready(loaded) => Some(loaded.release.summary.clone()),
            _ => None,
        };
        let status = self
            .revisions(cx)
            .into_iter()
            .find(|r| r.object == self.object)
            .map(|r| r.status)
            .or_else(|| summary.as_ref().map(|s| s.status.clone()))
            .unwrap_or_default();
        let revisions = self.revisions(cx);
        let weak = cx.entity().downgrade();
        let current = self.object.clone();
        let revision_menu = MenuButton::new("release-revision")
            .outline()
            .compact()
            .child(
                h_flex()
                    .gap(u(4.0))
                    .font_family(fonts::MONO)
                    .text_size(u(12.0))
                    .child(self.revision().map(|r| r.to_string()).unwrap_or_default())
                    .child(Icon::new(IconName::ChevronDown).size(11.0)),
            )
            .dropdown_menu(move |mut menu, _, _| {
                menu = menu.max_h(px(360.0)).scrollable(true);
                for revision in &revisions {
                    let weak = weak.clone();
                    let object = revision.object.clone();
                    menu = menu.item(
                        PopupMenuItem::new(format!(
                            "{} · {} · {} ago",
                            revision.revision,
                            revision.status,
                            widgets::ago(revision.modified)
                        ))
                        .checked(revision.object == current)
                        .on_click(move |_, _, cx| {
                            weak.update(cx, |this, cx| this.show_revision(object.clone(), cx))
                                .ok();
                        }),
                    );
                }
                menu
            });
        let header = h_flex()
            .flex_none()
            .h(u(44.0))
            .px(u(16.0))
            .gap(u(10.0))
            .border_b_1()
            .border_color(colors.border_variant)
            .overflow_hidden()
            .child(Icon::new(IconName::Anchor).size(15.0).color(colors.accent))
            .child(
                div()
                    .flex_none()
                    .font_family(fonts::MONO)
                    .text_size(u(14.0))
                    .font_weight(FontWeight::SEMIBOLD)
                    .child(self.name.clone()),
            )
            .child(
                div()
                    .flex_none()
                    .text_color(colors.text_dim)
                    .child(self.namespace.clone()),
            )
            .when(!status.is_empty(), |this| {
                this.child(div().flex_none().child(widgets::pill(
                    status.clone(),
                    crate::view::helm_status_tone(&status),
                    &colors,
                )))
            })
            .when_some(summary.clone(), |this, s| {
                this.child(
                    div()
                        .flex_1()
                        .min_w_0()
                        .truncate()
                        .font_family(fonts::MONO)
                        .text_size(u(12.0))
                        .text_color(colors.text_muted)
                        .child(format!(
                            "{}{} · revision",
                            s.chart_label(),
                            s.chart
                                .app_version
                                .as_ref()
                                .map(|v| format!(" · app {v}"))
                                .unwrap_or_default()
                        )),
                )
            })
            .when(summary.is_none(), |this| this.child(div().flex_1()))
            .child(revision_menu)
            .child(
                Button::new("release-commands")
                    .ghost()
                    .icon(IconName::Copy)
                    .label("Copy helm command…")
                    .on_click(cx.listener(|this, _, window, cx| this.copy_command(window, cx))),
            );
        let tab =
            |id: &'static str, tab: ReleaseTab, label: &'static str, cx: &mut Context<Self>| {
                let active = self.tab == tab;
                h_flex()
                    .id(id)
                    .h_full()
                    .px(u(10.0))
                    .cursor_pointer()
                    .text_color(if active {
                        colors.text
                    } else {
                        colors.text_muted
                    })
                    .border_b_2()
                    .border_color(if active {
                        colors.accent
                    } else {
                        gpui::transparent_black()
                    })
                    .child(label)
                    .on_click(cx.listener(move |this, _, _, cx| this.set_tab(tab, cx)))
            };
        let tabs = h_flex()
            .flex_none()
            .h(u(34.0))
            .px(u(8.0))
            .gap(u(2.0))
            .border_b_1()
            .border_color(colors.border_variant)
            .child(tab("release-tab-values", ReleaseTab::Values, "Values", cx))
            .child(tab(
                "release-tab-manifest",
                ReleaseTab::Manifest,
                "Manifest",
                cx,
            ))
            .child(tab("release-tab-notes", ReleaseTab::Notes, "Notes", cx))
            .child(tab(
                "release-tab-history",
                ReleaseTab::History,
                "History",
                cx,
            ))
            .child(tab(
                "release-tab-resources",
                ReleaseTab::Resources,
                "Resources",
                cx,
            ));
        let values_bar = (self.tab == ReleaseTab::Values).then(|| {
            h_flex()
                .flex_none()
                .h(u(36.0))
                .px(u(14.0))
                .gap(u(8.0))
                .border_b_1()
                .border_color(colors.border_variant)
                .text_size(u(12.5))
                .child(widgets::toggle_chip(
                    "values-user",
                    "User-supplied",
                    !self.all_values,
                    cx.listener(|this, _, _, cx| {
                        this.all_values = false;
                        cx.notify();
                    }),
                ))
                .child(widgets::toggle_chip(
                    "values-all",
                    "All (with chart defaults)",
                    self.all_values,
                    cx.listener(|this, _, _, cx| {
                        this.all_values = true;
                        cx.notify();
                    }),
                ))
                .child(div().flex_1())
                .child(
                    h_flex()
                        .gap(u(6.0))
                        .text_color(colors.text_dim)
                        .child(Icon::new(IconName::Lock).size(12.0).color(colors.text_dim))
                        .child(if self.reveal {
                            "Values shown: they can hold passwords."
                        } else {
                            "Values can hold passwords: strings and numbers are masked."
                        }),
                )
                .child(
                    Button::new("values-reveal")
                        .icon(if self.reveal {
                            IconName::EyeOff
                        } else {
                            IconName::Eye
                        })
                        .label(if self.reveal { "Mask" } else { "Reveal" })
                        .on_click(cx.listener(|this, _, _, cx| this.toggle_reveal(cx))),
                )
                .child(
                    Button::new("values-copy")
                        .ghost()
                        .icon(IconName::Copy)
                        .label("Copy")
                        .on_click(cx.listener(|this, _, _, cx| this.copy_values(cx))),
                )
        });
        let manifest_bar = (self.tab == ReleaseTab::Manifest).then(|| {
            h_flex()
                .flex_none()
                .h(u(30.0))
                .px(u(14.0))
                .gap(u(6.0))
                .border_b_1()
                .border_color(colors.border_variant)
                .text_size(u(12.0))
                .text_color(colors.text_dim)
                .child(Icon::new(IconName::Lock).size(12.0).color(colors.text_dim))
                .child("The data of Secrets in the manifest is masked.")
        });
        let body: AnyElement = match &self.state {
            State::Loading(_) => widgets::empty("Reading the release…", &colors),
            State::Failed(err) if err.is_empty() => widgets::empty("", &colors),
            State::Failed(err) => v_flex()
                .size_full()
                .items_center()
                .justify_center()
                .gap(u(10.0))
                .child(
                    Icon::new(IconName::TriangleAlert)
                        .size(22.0)
                        .color(colors.yellow),
                )
                .child(
                    div()
                        .max_w(u(560.0))
                        .text_color(colors.text_muted)
                        .child(err.clone()),
                )
                .into_any_element(),
            State::Ready(_) => match self.tab {
                ReleaseTab::History => self.render_history(cx),
                ReleaseTab::Resources => self.render_resources(cx),
                _ => match self.lines() {
                    Some(lines) => self.render_lines(lines, cx),
                    None => widgets::empty("", &colors),
                },
            },
        };
        let hints: Vec<(SharedString, SharedString)> = ActionRegistry::global(cx).hints(CONTEXT);
        v_flex()
            .key_context(CONTEXT)
            .track_focus(&self.focus)
            .size_full()
            .bg(colors.background)
            .text_color(colors.text)
            .font_family(fonts::UI)
            .text_size(u(sizes::UI_FONT))
            .on_action(
                cx.listener(|this, _: &ShowValues, _, cx| this.set_tab(ReleaseTab::Values, cx)),
            )
            .on_action(
                cx.listener(|this, _: &ShowManifest, _, cx| this.set_tab(ReleaseTab::Manifest, cx)),
            )
            .on_action(
                cx.listener(|this, _: &ShowNotes, _, cx| this.set_tab(ReleaseTab::Notes, cx)),
            )
            .on_action(
                cx.listener(|this, _: &ShowHistory, _, cx| this.set_tab(ReleaseTab::History, cx)),
            )
            .on_action(
                cx.listener(|this, _: &ShowResources, _, cx| {
                    this.set_tab(ReleaseTab::Resources, cx)
                }),
            )
            .on_action(cx.listener(|this, _: &ToggleReveal, _, cx| this.toggle_reveal(cx)))
            .on_action(cx.listener(|this, _: &OlderRevision, _, cx| this.step_revision(true, cx)))
            .on_action(cx.listener(|this, _: &NewerRevision, _, cx| this.step_revision(false, cx)))
            .on_action(
                cx.listener(|this, _: &CopyHelmCommand, window, cx| this.copy_command(window, cx)),
            )
            .child(header)
            .child(tabs)
            .children(values_bar)
            .children(manifest_bar)
            .child(div().flex_1().min_h_0().child(body))
            .child(kubyl_ui::KeyHints::new(hints))
    }
}
