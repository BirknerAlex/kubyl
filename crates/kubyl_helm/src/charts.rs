//! The Charts tab (board 20): the charts of the user's Helm repositories (`helm search repo`),
//! filtered by repository and search, OCI references typed into the search, Artifact Hub when
//! `helm.artifact_hub` is on; a chart's details (versions, `Chart.yaml`, README, default values,
//! CRDs) and Install. One tab per cluster: installs go there.

use std::collections::HashMap;
use std::ops::Range;
use std::sync::Arc;
use std::time::Duration;

use gpui::{
    AnyElement, App, AppContext as _, Context, Entity, FocusHandle, Focusable, FontWeight, Global,
    IntoElement, Render, ScrollStrategy, SharedString, Subscription, Task, UniformListScrollHandle,
    Window, actions, div, prelude::*, px, uniform_list,
};
use gpui_component::button::Button as MenuButton;
use gpui_component::input::{Input, InputEvent, InputState};
use gpui_component::menu::{DropdownMenu as _, PopupMenuItem};
use gpui_component::text::TextView;
use kubyl_core::actions::OpenView;
use kubyl_core::{
    ActionRegistry, ActionSpec, ActiveContext, ClusterId, ColumnDef, ColumnWidth, Gvr, ResourceRef,
    TabView, ViewKind, ViewRegistry, ViewRequest, spawn_kube,
};
use kubyl_helm_core::cli::{self, Probe};
use kubyl_helm_core::repo::{self, ChartDetails, ChartHit, ChartRef, Repository};
use kubyl_kube::ConnectionManager;
use kubyl_ui::{ActiveColors, Button, Colors, Icon, IconName, fonts, h_flex, sizes, u, v_flex};

use crate::cli::{CliState, HelmCli};
use crate::dialogs::{self, InstallRequest};
use crate::widgets;

/// `ViewKind::Custom` of the Charts tab; the target is the cluster (a list ref).
pub const VIEW_KIND: &str = "helm_charts";
pub const CONTEXT: &str = "HelmCharts";

actions!(
    helm_charts,
    [
        /// Installs the selected chart.
        InstallSelected,
        /// Updates the repositories' indexes.
        UpdateRepositories,
        /// Focuses the search.
        FocusSearch,
        /// Back from the search to the list.
        BlurSearch,
        /// Next chart.
        SelectNext,
        /// Previous chart.
        SelectPrevious,
    ]
);

/// Bumped when repositories change (the Charts tabs reload).
#[derive(Default)]
struct ChartsGeneration(u64);

impl Global for ChartsGeneration {}

/// Tells the Charts tabs that repositories were added, removed or updated.
pub fn repositories_changed(cx: &mut App) {
    cx.default_global::<ChartsGeneration>().0 += 1;
}

/// Opens (or focuses) the Charts tab of `cluster`.
pub fn open(cluster: &ClusterId, window: &mut Window, cx: &mut App) {
    window.dispatch_action(Box::new(OpenView(request(cluster))), cx);
}

pub fn request(cluster: &ClusterId) -> ViewRequest {
    ViewRequest::for_resource(
        ViewKind::Custom(VIEW_KIND.into()),
        ResourceRef::list(cluster.clone(), Gvr::new("", "", ""), None),
    )
}

pub(crate) fn init(cx: &mut App) {
    ViewRegistry::register(
        cx,
        ViewKind::Custom(VIEW_KIND.into()),
        |request, window, cx| {
            let cluster = request.target.as_ref()?.cluster.clone();
            Some(Box::new(cx.new(|cx| ChartsView::new(cluster, window, cx))))
        },
    );
    for (spec, keys) in [
        (
            ActionSpec::new("Helm Charts: Install…", InstallSelected).hint("Install…"),
            "i",
        ),
        (
            ActionSpec::new("Helm Charts: Update Repositories", UpdateRepositories)
                .hint("Update repositories"),
            "shift-u",
        ),
    ] {
        ActionRegistry::register(cx, spec.bind(keys, Some(CONTEXT)));
    }
    cx.bind_keys([
        gpui::KeyBinding::new("j", SelectNext, Some(CONTEXT)),
        gpui::KeyBinding::new("down", SelectNext, Some(CONTEXT)),
        gpui::KeyBinding::new("k", SelectPrevious, Some(CONTEXT)),
        gpui::KeyBinding::new("up", SelectPrevious, Some(CONTEXT)),
        gpui::KeyBinding::new("/", FocusSearch, Some(CONTEXT)),
        gpui::KeyBinding::new("escape", BlurSearch, Some("HelmChartsView > Input")),
        gpui::KeyBinding::new("down", BlurSearch, Some("HelmChartsView > Input")),
    ]);
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum DetailsTab {
    Readme,
    Values,
    Crds,
}

enum Load<T> {
    Idle,
    Loading(#[allow(dead_code)] Task<()>),
    Ready(T),
    Failed(String),
}

impl<T> Load<T> {
    fn ready(&self) -> Option<&T> {
        match self {
            Load::Ready(value) => Some(value),
            _ => None,
        }
    }
}

type DetailsKey = (ChartRef, Option<String>);
/// A chart's versions with their app versions, newest first.
type Versions = Vec<(String, Option<String>)>;

pub struct ChartsView {
    cluster: ClusterId,
    search: Entity<InputState>,
    repos: Load<Vec<Repository>>,
    charts: Load<Vec<ChartHit>>,
    hub: Load<Vec<ChartHit>>,
    hub_query: String,
    repo_filter: Option<String>,
    /// The rows the list shows (built on render, read by its `uniform_list`).
    rows: Vec<ChartHit>,
    selected: Option<ChartRef>,
    versions: HashMap<ChartRef, Load<Versions>>,
    version: Option<String>,
    details: HashMap<DetailsKey, Load<Arc<ChartDetails>>>,
    tab: DetailsTab,
    focus: FocusHandle,
    scroll: UniformListScrollHandle,
    generation: u64,
    _tasks: Vec<Task<()>>,
    _subscriptions: Vec<Subscription>,
}

impl ChartsView {
    pub fn new(cluster: ClusterId, window: &mut Window, cx: &mut Context<Self>) -> Self {
        let search = cx.new(|cx| {
            InputState::new(window, cx)
                .placeholder("Search charts, or oci://registry/path/chart:version")
        });
        let mut subscriptions = vec![cx.subscribe_in(
            &search,
            window,
            |this, _, event: &InputEvent, window, cx| match event {
                InputEvent::Change => {
                    this.scroll.scroll_to_item(0, ScrollStrategy::Top);
                    this.search_hub(cx);
                    cx.notify();
                }
                InputEvent::PressEnter { .. } => this.focus.focus(window, cx),
                _ => {}
            },
        )];
        if let Some(helm) = HelmCli::global(cx) {
            subscriptions.push(cx.observe(&helm, |this, _, cx| {
                this.load(cx);
                cx.notify();
            }));
        }
        subscriptions.push(cx.observe_global::<ChartsGeneration>(|this, cx| {
            let generation = cx.try_global::<ChartsGeneration>().map_or(0, |g| g.0);
            if generation != this.generation {
                this.generation = generation;
                this.repos = Load::Idle;
                this.charts = Load::Idle;
                this.versions.clear();
                this.load(cx);
            }
        }));
        let mut this = Self {
            cluster,
            search,
            repos: Load::Idle,
            charts: Load::Idle,
            hub: Load::Idle,
            hub_query: String::new(),
            repo_filter: None,
            rows: Vec::new(),
            selected: None,
            versions: HashMap::new(),
            version: None,
            details: HashMap::new(),
            tab: DetailsTab::Readme,
            focus: cx.focus_handle(),
            scroll: UniformListScrollHandle::new(),
            generation: cx.try_global::<ChartsGeneration>().map_or(0, |g| g.0),
            _tasks: Vec::new(),
            _subscriptions: subscriptions,
        };
        this.load(cx);
        this
    }

    /// Lists the repositories and their charts (once `helm` is found).
    fn load(&mut self, cx: &mut Context<Self>) {
        let Some(helm) = HelmCli::info(cx) else {
            return;
        };
        if matches!(self.repos, Load::Idle) {
            let helm = helm.clone();
            let work = spawn_kube(cx, async move {
                match cli::run(&helm, None, repo::list_repositories(), None, None).await {
                    Ok(output) => repo::parse_repositories(&output.stdout),
                    Err(err) if repo::is_no_repositories(&err.message) => Ok(Vec::new()),
                    Err(err) => Err(err.message),
                }
            });
            self.repos = Load::Loading(cx.spawn(async move |this, cx| {
                let result = work.await;
                this.update(cx, |this, cx| {
                    this.repos = match result {
                        Ok(repos) => Load::Ready(repos),
                        Err(err) => Load::Failed(err),
                    };
                    cx.notify();
                })
                .ok();
            }));
        }
        if matches!(self.charts, Load::Idle) {
            let work = spawn_kube(cx, async move {
                match cli::run(&helm, None, repo::search("", false), None, None).await {
                    Ok(output) => repo::parse_search(&output.stdout),
                    // No repositories: nothing to search (OCI references still work).
                    Err(err) if err.message.contains("no repositories") => Ok(Vec::new()),
                    Err(err) => Err(err.message),
                }
            });
            self.charts = Load::Loading(cx.spawn(async move |this, cx| {
                let result = work.await;
                this.update(cx, |this, cx| {
                    this.charts = match result {
                        Ok(charts) => Load::Ready(charts),
                        Err(err) => Load::Failed(err),
                    };
                    cx.notify();
                })
                .ok();
            }));
        }
    }

    fn query(&self, cx: &App) -> String {
        self.search.read(cx).value().trim().to_string()
    }

    /// Artifact Hub (opt-in), debounced.
    fn search_hub(&mut self, cx: &mut Context<Self>) {
        if !crate::cli::settings(cx).artifact_hub {
            return;
        }
        let query = self.query(cx);
        if query == self.hub_query {
            return;
        }
        if query.len() < 2 || query.starts_with("oci://") {
            // Not searched: no hits of an earlier query under this one.
            self.hub_query.clear();
            self.hub = Load::Idle;
            return;
        }
        self.hub_query = query.clone();
        let task = cx.spawn(async move |this, cx| {
            cx.background_executor()
                .timer(Duration::from_millis(400))
                .await;
            let Ok(work) = this.update(cx, |this, cx| {
                (this.hub_query == query).then(|| {
                    let query = query.clone();
                    spawn_kube(cx, async move {
                        repo::artifact_hub::search(repo::artifact_hub::BASE, &query).await
                    })
                })
            }) else {
                return;
            };
            let Some(work) = work else {
                return;
            };
            let result = work.await;
            this.update(cx, |this, cx| {
                this.hub = match result {
                    Ok(hits) => Load::Ready(hits),
                    Err(err) => Load::Failed(err),
                };
                cx.notify();
            })
            .ok();
        });
        self.hub = Load::Loading(task);
    }

    /// The rows for the search and repository filter.
    fn build_rows(&mut self, cx: &App) {
        let query = self.query(cx);
        if query.starts_with("oci://") {
            self.rows = repo::parse_reference(&query)
                .map(|(chart, version)| ChartHit {
                    source: "OCI reference".into(),
                    chart,
                    version: version.unwrap_or_default(),
                    app_version: None,
                    description: None,
                })
                .into_iter()
                .collect();
            return;
        }
        let words: Vec<String> = query
            .to_lowercase()
            .split_whitespace()
            .map(str::to_string)
            .collect();
        let matches = |hit: &ChartHit| {
            let haystack = format!(
                "{} {} {}",
                hit.chart.name(),
                hit.source,
                hit.description.as_deref().unwrap_or_default()
            )
            .to_lowercase();
            words.iter().all(|w| haystack.contains(w))
        };
        let mut rows: Vec<ChartHit> = self
            .charts
            .ready()
            .into_iter()
            .flatten()
            .filter(|hit| {
                self.repo_filter
                    .as_ref()
                    .is_none_or(|repo| hit.chart.repo() == Some(repo))
            })
            .filter(|hit| matches(hit))
            .cloned()
            .collect();
        // Name matches first, then by name.
        let first = words.first().cloned().unwrap_or_default();
        rows.sort_by(|a, b| {
            let rank = |h: &ChartHit| {
                if first.is_empty() || h.chart.name().starts_with(&first) {
                    0
                } else if h.chart.name().contains(&first) {
                    1
                } else {
                    2
                }
            };
            (rank(a), a.chart.name(), &a.source).cmp(&(rank(b), b.chart.name(), &b.source))
        });
        if self.repo_filter.is_none() && !query.is_empty() {
            rows.extend(self.hub.ready().into_iter().flatten().cloned());
        }
        self.rows = rows;
    }

    fn selected_hit(&self) -> Option<&ChartHit> {
        let selected = self.selected.as_ref()?;
        self.rows.iter().find(|h| &h.chart == selected)
    }

    fn select(&mut self, chart: ChartRef, cx: &mut Context<Self>) {
        if self.selected.as_ref() != Some(&chart) {
            self.version = None;
            self.tab = DetailsTab::Readme;
        }
        self.selected = Some(chart);
        self.load_selected(cx);
        cx.notify();
    }

    fn move_selection(&mut self, delta: isize, cx: &mut Context<Self>) {
        if self.rows.is_empty() {
            return;
        }
        let current = self
            .selected
            .as_ref()
            .and_then(|s| self.rows.iter().position(|h| &h.chart == s));
        let next = match current {
            None => 0,
            Some(i) => (i as isize + delta).clamp(0, self.rows.len() as isize - 1) as usize,
        };
        self.scroll.scroll_to_item(next, ScrollStrategy::Nearest);
        let chart = self.rows[next].chart.clone();
        self.select(chart, cx);
    }

    /// Loads the selected chart's versions and details.
    fn load_selected(&mut self, cx: &mut Context<Self>) {
        let Some(helm) = HelmCli::info(cx) else {
            return;
        };
        let Some(chart) = self.selected.clone() else {
            return;
        };
        if let ChartRef::Repo {
            repo: repo_name,
            name,
        } = &chart
            && !self.versions.contains_key(&chart)
        {
            let (helm, repo_name, name) = (helm.clone(), repo_name.clone(), name.clone());
            let key = chart.clone();
            let work = spawn_kube(cx, async move {
                let output = cli::run(&helm, None, repo::versions(&repo_name, &name), None, None)
                    .await
                    .map_err(|e| e.message)?;
                repo::parse_versions(&output.stdout, &repo_name, &name)
            });
            let task = cx.spawn(async move |this, cx| {
                let result = work.await;
                this.update(cx, |this, cx| {
                    this.versions.insert(
                        key,
                        match result {
                            Ok(hits) => Load::Ready(
                                hits.into_iter()
                                    .map(|h| (h.version, h.app_version))
                                    .collect(),
                            ),
                            Err(err) => Load::Failed(err),
                        },
                    );
                    cx.notify();
                })
                .ok();
            });
            self.versions.insert(chart.clone(), Load::Loading(task));
        }
        let version = self.version.clone().or_else(|| {
            // OCI references carry their version in the row.
            self.selected_hit()
                .filter(|h| matches!(h.chart, ChartRef::Oci { .. }))
                .map(|h| h.version.clone())
                .filter(|v| !v.is_empty())
        });
        let key: DetailsKey = (chart.clone(), version.clone());
        if self.details.contains_key(&key) {
            return;
        }
        let task_key = key.clone();
        let work = spawn_kube(cx, async move {
            repo::details(&helm, &chart, version.as_deref()).await
        });
        let task = cx.spawn(async move |this, cx| {
            let result = work.await;
            this.update(cx, |this, cx| {
                this.details.insert(
                    task_key,
                    match result {
                        Ok(details) => Load::Ready(Arc::new(details)),
                        Err(err) => Load::Failed(err),
                    },
                );
                cx.notify();
            })
            .ok();
        });
        self.details.insert(key, Load::Loading(task));
    }

    fn install(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        // No Helm write on a read-only cluster (the button and its hint are hidden).
        if crate::cli::read_only(&self.cluster, cx) {
            return;
        }
        let Some(hit) = self.selected_hit().cloned() else {
            return;
        };
        // The version shown: the one picked, else the search's (the latest that isn't a
        // pre-release, or an OCI reference's tag).
        let version = self
            .version
            .clone()
            .or_else(|| (!hit.version.is_empty()).then(|| hit.version.clone()));
        dialogs::open_install(
            InstallRequest {
                cluster: Some(self.cluster.clone()),
                chart: Some(hit.chart),
                version,
                namespace: None,
            },
            window,
            cx,
        );
    }

    fn update_repositories(&mut self, cx: &mut Context<Self>) {
        let Some(helm) = HelmCli::info(cx) else {
            return;
        };
        let work = spawn_kube(cx, async move {
            cli::run(&helm, None, repo::update_repositories(&[]), None, None).await
        });
        self._tasks.retain(|t| !t.is_ready());
        self._tasks.push(cx.spawn(async move |_, cx| {
            let result = work.await;
            cx.update(|cx| {
                match result {
                    Ok(_) => kubyl_core::NotificationCenter::push(
                        cx,
                        kubyl_core::Notification::info("Updated the Helm repositories."),
                    ),
                    Err(err) => kubyl_core::NotificationCenter::push(
                        cx,
                        kubyl_core::Notification::error(format!(
                            "Updating the repositories failed: {}",
                            err.message
                        )),
                    ),
                }
                repositories_changed(cx);
            });
        }));
        kubyl_core::NotificationCenter::push(
            cx,
            kubyl_core::Notification::info("Updating the Helm repositories…"),
        );
    }

    fn columns() -> Vec<ColumnDef> {
        vec![
            ColumnDef::new(
                "chart",
                "Chart",
                ColumnWidth::Flex {
                    weight: 1.0,
                    min: 150.0,
                },
            ),
            ColumnDef::new("repo", "Repository", ColumnWidth::Fixed(150.0)),
            ColumnDef::new("version", "Version", ColumnWidth::Fixed(84.0)),
            ColumnDef::new("app", "App", ColumnWidth::Fixed(84.0)),
            ColumnDef::new(
                "description",
                "Description",
                ColumnWidth::Flex {
                    weight: 1.4,
                    min: 120.0,
                },
            ),
        ]
    }

    fn render_rows(&mut self, range: Range<usize>, cx: &mut Context<Self>) -> Vec<AnyElement> {
        let colors = cx.colors().clone();
        let columns = Self::columns();
        range
            .filter_map(|index| {
                let hit = self.rows.get(index)?.clone();
                let selected = self.selected.as_ref() == Some(&hit.chart);
                let chart = hit.chart.clone();
                Some(
                    widgets::row(("chart-row", index), selected, 32.0, &colors)
                        .children(columns.iter().map(|def| {
                            let dim = |text: String| {
                                div()
                                    .truncate()
                                    .font_family(fonts::MONO)
                                    .text_size(u(12.0))
                                    .text_color(colors.text_muted)
                                    .child(text)
                                    .into_any_element()
                            };
                            let cell = match def.id.as_ref() {
                                "chart" => widgets::mono(hit.chart.name().to_string()),
                                "repo" => dim(hit.source.clone()),
                                "version" => widgets::mono(hit.version.clone()),
                                "app" => dim(hit.app_version.clone().unwrap_or_default()),
                                _ => div()
                                    .truncate()
                                    .text_size(u(12.0))
                                    .text_color(colors.text_dim)
                                    .child(match &hit.chart {
                                        ChartRef::Oci { reference } => reference.clone(),
                                        _ => hit.description.clone().unwrap_or_default(),
                                    })
                                    .into_any_element(),
                            };
                            widgets::column_cell(def).child(cell)
                        }))
                        .on_click(
                            cx.listener(move |this, event: &gpui::ClickEvent, window, cx| {
                                this.focus.focus(window, cx);
                                this.select(chart.clone(), cx);
                                if event.click_count() == 2 {
                                    this.install(window, cx);
                                }
                            }),
                        )
                        .into_any_element(),
                )
            })
            .collect()
    }

    fn render_repos(&self, cx: &mut Context<Self>) -> AnyElement {
        let colors = cx.colors().clone();
        let charts = self.charts.ready().cloned().unwrap_or_default();
        let count = |repo: Option<&str>| {
            charts
                .iter()
                .filter(|h| repo.is_none_or(|r| h.chart.repo() == Some(r)))
                .count()
        };
        let row = |id: (&'static str, usize), label: String, n: usize, on: bool, mono: bool| {
            h_flex()
                .id(id)
                .h(u(24.0))
                .px(u(8.0))
                .rounded(u(5.0))
                .cursor_pointer()
                .text_size(u(12.5))
                .when(on, |this| this.bg(colors.selection).text_color(colors.text))
                .when(!on, |this| {
                    this.text_color(colors.text_muted)
                        .hover(|s| s.bg(colors.hover))
                })
                .child(
                    div()
                        .flex_1()
                        .min_w_0()
                        .truncate()
                        .when(mono, |this| this.font_family(fonts::MONO))
                        .child(label),
                )
                .child(
                    div()
                        .font_family(fonts::MONO)
                        .text_size(u(11.0))
                        .text_color(colors.text_dim)
                        .child(n.to_string()),
                )
        };
        let mut list = v_flex().gap(u(1.0)).child(
            row(
                ("charts-repo-all", 0),
                "All repositories".into(),
                count(None),
                self.repo_filter.is_none(),
                false,
            )
            .on_click(cx.listener(|this, _, _, cx| {
                this.repo_filter = None;
                cx.notify();
            })),
        );
        if let Some(repos) = self.repos.ready() {
            for (i, repository) in repos.iter().enumerate() {
                let name = repository.name.clone();
                list = list.child(
                    row(
                        ("charts-repo", i),
                        repository.name.clone(),
                        count(Some(&repository.name)),
                        self.repo_filter.as_deref() == Some(&repository.name),
                        true,
                    )
                    .on_click(cx.listener(move |this, _, _, cx| {
                        this.repo_filter = Some(name.clone());
                        cx.notify();
                    })),
                );
            }
        }
        let hub = if crate::cli::settings(cx).artifact_hub {
            "Artifact Hub search: on (helm.artifact_hub)."
        } else {
            "Artifact Hub search: off (helm.artifact_hub)."
        };
        v_flex()
            .flex_none()
            .w(u(200.0))
            .h_full()
            .px(u(10.0))
            .py(u(12.0))
            .gap(u(6.0))
            .border_r_1()
            .border_color(colors.border_variant)
            .child(
                div()
                    .px(u(8.0))
                    .child(widgets::dialog_title("Repositories", &colors)),
            )
            .child(
                div()
                    .id("charts-repos")
                    .flex_1()
                    .min_h_0()
                    .overflow_y_scroll()
                    .child(list),
            )
            .child(div().px(u(8.0)).child(widgets::muted(
                format!("From Helm's own repositories.yaml (helm env). {hub}"),
                &colors,
            )))
            .into_any_element()
    }

    fn render_list(&mut self, cx: &mut Context<Self>) -> AnyElement {
        let colors = cx.colors().clone();
        let body: AnyElement = if self.rows.is_empty() {
            let message = match (&self.charts, self.repos.ready()) {
                (Load::Loading(_) | Load::Idle, _) => "Reading the repositories' indexes…".to_string(),
                (Load::Failed(err), _) => err.clone(),
                (_, Some(repos)) if repos.is_empty() => {
                    "No Helm repositories yet: add one (Repositories…), or type an oci:// reference.".into()
                }
                _ => "No charts match the search.".into(),
            };
            widgets::empty(message, &colors)
        } else {
            uniform_list(
                "chart-rows",
                self.rows.len(),
                cx.processor(|this, range: Range<usize>, _, cx| this.render_rows(range, cx)),
            )
            .flex_1()
            .track_scroll(&self.scroll)
            .into_any_element()
        };
        let hub_note = match &self.hub {
            Load::Loading(_) => Some("Searching Artifact Hub…".to_string()),
            Load::Failed(err) => Some(err.clone()),
            _ => None,
        };
        v_flex()
            .flex_1()
            .min_w_0()
            .h_full()
            .child(widgets::header(&Self::columns(), &colors))
            .child(body)
            .children(hub_note.map(|note| {
                div()
                    .px(u(12.0))
                    .py(u(4.0))
                    .border_t_1()
                    .border_color(colors.border_variant)
                    .child(widgets::muted(note, &colors))
            }))
            .into_any_element()
    }

    fn render_details(&mut self, cx: &mut Context<Self>) -> Option<AnyElement> {
        let colors = cx.colors().clone();
        let hit = self.selected_hit()?.clone();
        let versions = self
            .versions
            .get(&hit.chart)
            .and_then(Load::ready)
            .cloned()
            .unwrap_or_default();
        let version = self.version.clone().or_else(|| {
            matches!(hit.chart, ChartRef::Oci { .. })
                .then(|| hit.version.clone())
                .filter(|v| !v.is_empty())
        });
        let details = self.details.get(&(hit.chart.clone(), version.clone()));
        let name = hit.chart.name().to_string();
        let initials: String = name
            .split(['-', '_'])
            .filter_map(|p| p.chars().next())
            .take(2)
            .collect::<String>()
            .to_uppercase();
        let weak = cx.entity().downgrade();
        let shown = version.clone().unwrap_or_else(|| hit.version.clone());
        // Unpicked: the search's version, the newest that isn't a pre-release.
        let shown_note = if version.is_none() { "latest" } else { "" };
        let shown_version = if version.is_none() {
            shown.clone()
        } else {
            dialogs::version_label(&versions, &shown)
        };
        let version_menu = MenuButton::new("chart-version")
            .outline()
            .child(
                h_flex()
                    .gap(u(8.0))
                    .child(
                        div()
                            .font_family(fonts::MONO)
                            .text_size(u(12.0))
                            .child(shown_version),
                    )
                    .child(
                        div()
                            .text_size(u(11.5))
                            .text_color(colors.text_dim)
                            .child(shown_note),
                    )
                    .child(Icon::new(IconName::ChevronDown).size(11.0)),
            )
            .dropdown_menu({
                let items: Vec<(String, String)> = versions
                    .iter()
                    .map(|(v, app)| {
                        let label = dialogs::version_label(&versions, v);
                        let label = match app {
                            Some(app) => format!("{label} · app {app}"),
                            None => label,
                        };
                        (v.clone(), label)
                    })
                    .collect();
                let current = shown.clone();
                move |mut menu, _, _| {
                    menu = menu.max_h(px(320.0)).scrollable(true);
                    for (v, label) in &items {
                        let weak = weak.clone();
                        let pick = Some(v.clone());
                        menu = menu.item(
                            PopupMenuItem::new(label.clone())
                                .checked(&current == v)
                                .on_click(move |_, _, cx| {
                                    let pick = pick.clone();
                                    weak.update(cx, |this, cx| {
                                        this.version = pick;
                                        this.load_selected(cx);
                                        cx.notify();
                                    })
                                    .ok();
                                }),
                        );
                    }
                    menu
                }
            });
        // Read-only clusters show no Helm write action.
        let install = (!crate::cli::read_only(&self.cluster, cx)).then(|| {
            Button::new("chart-install")
                .primary()
                .icon(IconName::Download)
                .label("Install…")
                .on_click(cx.listener(|this, _, window, cx| this.install(window, cx)))
        });
        let mut kv: Vec<(&'static str, AnyElement)> = Vec::new();
        let body: AnyElement = match details {
            Some(Load::Ready(details)) => {
                let chart = &details.chart;
                if let Some(app) = &chart.app_version {
                    kv.push(("App version", widgets::kv_mono(app.clone())));
                }
                if !versions.is_empty() {
                    let shown: Vec<String> =
                        versions.iter().take(5).map(|(v, _)| v.clone()).collect();
                    let more = versions.len().saturating_sub(5);
                    kv.push((
                        "Versions",
                        widgets::kv_text(if more > 0 {
                            format!("{} · and {more} more", shown.join(" · "))
                        } else {
                            shown.join(" · ")
                        }),
                    ));
                }
                if let Some(t) = &chart.chart_type {
                    kv.push(("Type", widgets::kv_text(t.clone())));
                }
                if let Some(home) = &chart.home {
                    let url = home.clone();
                    kv.push((
                        "Home",
                        widgets::link("chart-home", home.clone(), &colors, move |_, _, cx| {
                            widgets::open_url(&url, cx)
                        })
                        .into_any_element(),
                    ));
                }
                if !chart.maintainers.is_empty() {
                    kv.push((
                        "Maintainers",
                        widgets::kv_text(
                            chart
                                .maintainers
                                .iter()
                                .map(|m| m.name.clone())
                                .collect::<Vec<_>>()
                                .join(", "),
                        ),
                    ));
                }
                if !chart.keywords.is_empty() {
                    kv.push(("Keywords", widgets::kv_text(chart.keywords.join(", "))));
                }
                if let Some(kube) = &chart.kube_version {
                    kv.push(("Kubernetes", widgets::kv_mono(kube.clone())));
                }
                if !details.crds.is_empty() {
                    kv.push(("CRDs", widgets::kv_mono(details.crds.join(", "))));
                }
                kv.push((
                    "Values schema",
                    widgets::kv_text(if details.schema.is_some() {
                        "yes"
                    } else {
                        "no"
                    }),
                ));
                if chart.deprecated {
                    kv.push((
                        "Deprecated",
                        widgets::kv_text("yes: the chart is deprecated"),
                    ));
                }
                match self.tab {
                    DetailsTab::Readme => match &details.readme {
                        Some(readme) => div()
                            .p(u(14.0))
                            .text_size(u(12.5))
                            .child(TextView::markdown(
                                SharedString::from(format!("chart-readme-{name}")),
                                readme.clone(),
                            ))
                            .into_any_element(),
                        None => widgets::empty("The chart has no README.", &colors),
                    },
                    DetailsTab::Values => v_flex()
                        .py(u(6.0))
                        .children(
                            kubyl_helm_core::present::text_lines(&details.values)
                                .iter()
                                .enumerate()
                                .map(|(i, line)| widgets::yaml_line(i + 1, line, &colors)),
                        )
                        .into_any_element(),
                    DetailsTab::Crds if details.crds.is_empty() => {
                        widgets::empty("The chart has no CRDs in crds/.", &colors)
                    }
                    DetailsTab::Crds => v_flex()
                        .p(u(14.0))
                        .gap(u(4.0))
                        .children(details.crds.iter().map(|crd| {
                            widgets::note(IconName::File, colors.yellow, crd.clone(), &colors)
                        }))
                        .child(widgets::muted(
                            "Helm installs these once and never upgrades or deletes them.",
                            &colors,
                        ))
                        .into_any_element(),
                }
            }
            Some(Load::Failed(err)) => div()
                .p(u(14.0))
                .child(widgets::note(
                    IconName::TriangleAlert,
                    colors.yellow,
                    err.clone(),
                    &colors,
                ))
                .into_any_element(),
            _ => widgets::empty("Pulling the chart…", &colors),
        };
        let tab =
            |id: &'static str, label: &'static str, which: DetailsTab, cx: &mut Context<Self>| {
                let active = self.tab == which;
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
                    .on_click(cx.listener(move |this, _, _, cx| {
                        this.tab = which;
                        cx.notify();
                    }))
            };
        let tabs = h_flex()
            .flex_none()
            .h(u(32.0))
            .px(u(8.0))
            .gap(u(2.0))
            .border_b_1()
            .border_color(colors.border_variant)
            .child(tab("chart-tab-readme", "README", DetailsTab::Readme, cx))
            .child(tab(
                "chart-tab-values",
                "Default values",
                DetailsTab::Values,
                cx,
            ))
            .child(tab("chart-tab-crds", "CRDs", DetailsTab::Crds, cx));
        Some(
            v_flex()
                .flex_none()
                .w(u(400.0))
                .h_full()
                .bg(colors.panel)
                .border_l_1()
                .border_color(colors.border)
                .child(
                    h_flex()
                        .flex_none()
                        .items_start()
                        .gap(u(10.0))
                        .px(u(14.0))
                        .py(u(12.0))
                        .border_b_1()
                        .border_color(colors.border_variant)
                        .child(
                            div()
                                .flex_none()
                                .size(u(34.0))
                                .rounded(u(6.0))
                                .bg(colors.accent.opacity(0.14))
                                .border_1()
                                .border_color(colors.accent.opacity(0.35))
                                .flex()
                                .items_center()
                                .justify_center()
                                .text_color(colors.accent)
                                .font_weight(FontWeight::SEMIBOLD)
                                .child(initials),
                        )
                        .child(
                            v_flex()
                                .flex_1()
                                .min_w_0()
                                .child(
                                    div()
                                        .font_family(fonts::MONO)
                                        .text_size(u(14.0))
                                        .font_weight(FontWeight::SEMIBOLD)
                                        .child(name.clone()),
                                )
                                .child(div().text_size(u(11.5)).text_color(colors.text_dim).child(
                                    format!(
                                            "{}{}",
                                            hit.source,
                                            hit.description
                                                .as_ref()
                                                .map(|d| format!(" · {d}"))
                                                .unwrap_or_default()
                                        ),
                                )),
                        ),
                )
                .child(
                    v_flex()
                        .id("chart-details")
                        .flex_1()
                        .min_h_0()
                        .overflow_y_scroll()
                        .child(
                            h_flex()
                                .gap(u(8.0))
                                .px(u(14.0))
                                .py(u(10.0))
                                .border_b_1()
                                .border_color(colors.border_variant)
                                .children(install)
                                .when(!versions.is_empty(), |this| this.child(version_menu)),
                        )
                        .when(!kv.is_empty(), |this| {
                            this.child(
                                div()
                                    .px(u(14.0))
                                    .py(u(10.0))
                                    .border_b_1()
                                    .border_color(colors.border_variant)
                                    .child(widgets::kv(kv, &colors)),
                            )
                        })
                        .child(tabs)
                        .child(body),
                )
                .into_any_element(),
        )
    }

    fn summary(&self) -> String {
        let charts = self.charts.ready().map(Vec::len).unwrap_or(0);
        let repos = self.repos.ready().map(Vec::len).unwrap_or(0);
        format!("{charts} charts · {repos} repositories")
    }

    fn render_toolbar(&self, cx: &mut Context<Self>) -> AnyElement {
        let colors = cx.colors().clone();
        let cluster = ConnectionManager::try_global(cx)
            .map(|m| m.read(cx).display_name(&self.cluster).to_string())
            .unwrap_or_else(|| self.cluster.to_string());
        h_flex()
            .flex_none()
            .h(u(40.0))
            .px(u(12.0))
            .gap(u(8.0))
            .border_b_1()
            .border_color(colors.border_variant)
            .overflow_hidden()
            .child(Icon::new(IconName::Anchor).size(14.0).color(colors.accent))
            .child(
                div()
                    .flex_none()
                    .font_weight(FontWeight::MEDIUM)
                    .child("Charts"),
            )
            .child(div().text_color(colors.text_dim).child("·"))
            .child(
                div()
                    .flex_1()
                    .min_w_0()
                    .truncate()
                    .text_color(colors.text_dim)
                    .child(format!("{} · installs into {cluster}", self.summary())),
            )
            .child(
                h_flex()
                    .id("charts-search")
                    .flex_none()
                    .w(u(330.0))
                    .h(u(sizes::CONTROL))
                    .px(u(8.0))
                    .gap(u(7.0))
                    .rounded(u(5.0))
                    .bg(colors.input_background)
                    .border_1()
                    .border_color(colors.border)
                    .text_size(u(12.5))
                    .child(
                        Icon::new(IconName::Search)
                            .size(12.0)
                            .color(colors.text_dim),
                    )
                    .child(
                        div().flex_1().min_w_0().child(
                            Input::new(&self.search)
                                .appearance(false)
                                .text_size(u(12.5)),
                        ),
                    ),
            )
            .child(
                Button::new("charts-update")
                    .ghost()
                    .icon(IconName::RefreshCw)
                    .label("Update")
                    .on_click(cx.listener(|this, _, _, cx| this.update_repositories(cx))),
            )
            .child(
                Button::new("charts-repositories")
                    .icon(IconName::Settings)
                    .label("Repositories…")
                    .on_click(|_, window, cx| dialogs::open_repositories(window, cx)),
            )
            .into_any_element()
    }
}

impl Focusable for ChartsView {
    fn focus_handle(&self, _: &App) -> FocusHandle {
        self.focus.clone()
    }
}

impl TabView for ChartsView {
    fn tab_title(&self, cx: &App) -> SharedString {
        let active =
            ActiveContext::global(cx).cluster.as_ref().map(|c| &c.id) == Some(&self.cluster);
        if active {
            "Charts".into()
        } else {
            let name = ConnectionManager::try_global(cx)
                .map(|m| m.read(cx).display_name(&self.cluster).to_string())
                .unwrap_or_else(|| self.cluster.to_string());
            format!("Charts · {name}").into()
        }
    }

    fn tab_icon(&self, _: &App) -> Option<SharedString> {
        Some(IconName::Anchor.path())
    }

    fn view_request(&self, _: &App) -> Option<ViewRequest> {
        Some(request(&self.cluster))
    }
}

impl Render for ChartsView {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let colors: Colors = cx.colors().clone();
        let state = HelmCli::global(cx).map(|h| h.read(cx).state().clone());
        let ready = matches!(&state, Some(CliState::Done(probe)) if matches!(probe.as_ref(), Probe::Ready(_)));
        let toolbar = self.render_toolbar(cx);
        let body: AnyElement = if !ready {
            div()
                .flex_1()
                .p(u(16.0))
                .child(div().max_w(u(640.0)).child(dialogs::missing::body(cx)))
                .into_any_element()
        } else {
            self.build_rows(cx);
            if self.selected.is_none()
                && let Some(first) = self.rows.first().map(|h| h.chart.clone())
            {
                self.select(first, cx);
            }
            let repos = self.render_repos(cx);
            let list = self.render_list(cx);
            let details = self.render_details(cx);
            h_flex()
                .flex_1()
                .min_h_0()
                .items_start()
                .child(repos)
                .child(
                    div()
                        .key_context(CONTEXT)
                        .track_focus(&self.focus)
                        .flex()
                        .flex_1()
                        .min_w_0()
                        .h_full()
                        .on_action(
                            cx.listener(|this, _: &SelectNext, _, cx| this.move_selection(1, cx)),
                        )
                        .on_action(cx.listener(|this, _: &SelectPrevious, _, cx| {
                            this.move_selection(-1, cx)
                        }))
                        .on_action(cx.listener(|this, _: &InstallSelected, window, cx| {
                            this.install(window, cx)
                        }))
                        .on_action(cx.listener(|this, _: &UpdateRepositories, _, cx| {
                            this.update_repositories(cx)
                        }))
                        .child(list)
                        .children(details),
                )
                .into_any_element()
        };
        let mut hints: Vec<(SharedString, SharedString)> = vec![("j/k".into(), "Select".into())];
        hints.extend(ActionRegistry::global(cx).hints(CONTEXT));
        let mut hints = crate::visible_hints(hints, !crate::cli::read_only(&self.cluster, cx));
        hints.push(("/".into(), "Search".into()));
        v_flex()
            .key_context("HelmChartsView")
            .size_full()
            .bg(colors.background)
            .text_color(colors.text)
            .font_family(fonts::UI)
            .text_size(u(sizes::UI_FONT))
            .on_action(cx.listener(|this, _: &FocusSearch, window, cx| {
                let focus = this.search.read(cx).focus_handle(cx);
                focus.focus(window, cx);
            }))
            .on_action(cx.listener(|this, _: &BlurSearch, window, cx| this.focus.focus(window, cx)))
            .child(toolbar)
            .child(body)
            .child(kubyl_ui::KeyHints::new(hints))
    }
}
