//! Projects (`AppProject`): source repositories and namespaces, destinations, cluster and
//! namespace resource allow/deny lists, roles, and sync windows with whether one blocks syncing
//! right now.

use std::ops::Range;
use std::sync::Arc;

use gpui::{
    AnyElement, App, Context, FocusHandle, Focusable, FontWeight, IntoElement, Render,
    ScrollStrategy, SharedString, Subscription, UniformListScrollHandle, Window, div, prelude::*,
    uniform_list,
};
use kubyl_core::{
    ClusterId, ColumnDef, ColumnWidth, Gvr, ResourceRef, TabView, ViewKind, ViewRequest,
};
use kubyl_resources::{ResourceStores, StoreHandle, StoreKey, StoreStatus};
use kubyl_ui::{
    ActiveColors, Chip, Colors, Icon, IconName, KeyHints, fonts, h_flex, sizes, u, v_flex,
};

use crate::model::{Application, GROUP, GroupKind, Project, SyncWindow};
use crate::nav::{self, Move, TABLE};
use crate::state;
use crate::widgets;
use crate::windows;

pub const VIEW_KIND: &str = "argocd_projects";
const CONTEXT: &str = "ArgoProjects";

fn columns() -> Vec<ColumnDef> {
    vec![
        ColumnDef::new(
            "name",
            "Name",
            ColumnWidth::Flex {
                weight: 0.8,
                min: 140.0,
            },
        ),
        ColumnDef::new(
            "description",
            "Description",
            ColumnWidth::Flex {
                weight: 1.2,
                min: 160.0,
            },
        ),
        ColumnDef::new("sources", "Sources", ColumnWidth::Fixed(90.0)),
        ColumnDef::new("destinations", "Destinations", ColumnWidth::Fixed(110.0)),
        ColumnDef::new("apps", "Apps", ColumnWidth::Fixed(60.0)),
        ColumnDef::new("windows", "Sync windows", ColumnWidth::Fixed(150.0)),
    ]
}

struct Row {
    project: Project,
    apps: usize,
}

/// A window's state now: active or not.
fn window_line(window: &SyncWindow, colors: &Colors) -> AnyElement {
    let active = windows::is_active(window, jiff::Timestamp::now());
    let color = match (window.kind.as_str(), active) {
        ("deny", true) => colors.red,
        ("allow", true) => colors.green,
        _ => colors.text_dim,
    };
    let mut applies: Vec<String> = Vec::new();
    if !window.applications.is_empty() {
        applies.push(format!("apps {}", window.applications.join(", ")));
    }
    if !window.namespaces.is_empty() {
        applies.push(format!("namespaces {}", window.namespaces.join(", ")));
    }
    if !window.clusters.is_empty() {
        applies.push(format!("clusters {}", window.clusters.join(", ")));
    }
    v_flex()
        .text_size(u(12.0))
        .child(
            h_flex()
                .gap(u(6.0))
                .child(widgets::pill(
                    format!(
                        "{} {}",
                        window.kind,
                        if active { "· active now" } else { "" }
                    ),
                    color,
                ))
                .child(
                    div()
                        .font_family(fonts::MONO)
                        .text_size(u(11.5))
                        .child(format!("{} for {}", window.schedule, window.duration)),
                ),
        )
        .child(div().pl(u(13.0)).text_color(colors.text_dim).child(format!(
                    "{}{}{}",
                    applies.join(" · "),
                    if window.manual_sync { " · manual sync allowed" } else { "" },
                    window.time_zone.as_ref().map(|tz| format!(" · {tz}")).unwrap_or_default()
                )))
        .into_any_element()
}

pub struct ProjectsView {
    cluster: ClusterId,
    namespace: Option<String>,
    gvr: Option<Gvr>,
    projects: Option<StoreHandle>,
    apps: Option<StoreHandle>,
    rows: Vec<Arc<Row>>,
    selected: Option<String>,
    focus: FocusHandle,
    scroll: UniformListScrollHandle,
    _observers: Vec<Subscription>,
    _subscription: Option<Subscription>,
}

impl ProjectsView {
    pub fn new(target: ResourceRef, _: &mut Window, cx: &mut Context<Self>) -> Self {
        let subscription = kubyl_kube::ConnectionManager::try_global(cx).map(|manager| {
            let cluster = target.cluster.clone();
            cx.subscribe(&manager, move |this: &mut Self, _, event: &kubyl_kube::ConnectionEvent, cx| {
                if matches!(event, kubyl_kube::ConnectionEvent::DiscoveryChanged(id) if *id == cluster) {
                    this.sync_stores(cx);
                }
            })
        });
        let mut this = Self {
            cluster: target.cluster.clone(),
            namespace: target.namespace.clone(),
            gvr: None,
            projects: None,
            apps: None,
            rows: Vec::new(),
            selected: None,
            focus: cx.focus_handle(),
            scroll: UniformListScrollHandle::new(),
            _observers: Vec::new(),
            _subscription: subscription,
        };
        this.sync_stores(cx);
        this
    }

    fn sync_stores(&mut self, cx: &mut Context<Self>) {
        let gvr = state::resource(&self.cluster, "appprojects", cx).map(|(g, _)| g);
        if gvr == self.gvr && self.projects.is_some() {
            return;
        }
        self.gvr = gvr.clone();
        self.projects = gvr.map(|gvr| {
            ResourceStores::acquire(
                cx,
                StoreKey::new(self.cluster.clone(), gvr, self.namespace.clone()),
            )
        });
        self.apps = state::resource(&self.cluster, "applications", cx).map(|(gvr, _)| {
            ResourceStores::acquire(cx, StoreKey::new(self.cluster.clone(), gvr, None))
        });
        self._observers = [&self.projects, &self.apps]
            .into_iter()
            .flatten()
            .map(|h| cx.observe(h.entity(), |this, _, cx| this.refresh(cx)))
            .collect();
        self.refresh(cx);
    }

    fn refresh(&mut self, cx: &mut Context<Self>) {
        let apps: Vec<Application> = self
            .apps
            .as_ref()
            .map(|h| {
                h.read(cx)
                    .objects()
                    .values()
                    .filter_map(|o| Application::parse(o))
                    .collect()
            })
            .unwrap_or_default();
        let mut rows: Vec<Arc<Row>> = self
            .projects
            .as_ref()
            .map(|h| {
                h.read(cx)
                    .objects()
                    .values()
                    .filter_map(|o| Project::parse(o))
                    .map(|project| {
                        let count = apps
                            .iter()
                            .filter(|a| a.spec.project == project.metadata.name)
                            .count();
                        Arc::new(Row {
                            project,
                            apps: count,
                        })
                    })
                    .collect()
            })
            .unwrap_or_default();
        rows.sort_by(|a, b| a.project.metadata.name.cmp(&b.project.metadata.name));
        self.rows = rows;
        if self.selected.is_none() {
            self.selected = self.rows.first().map(|r| r.project.metadata.name.clone());
        }
        cx.notify();
    }

    fn selected_index(&self) -> Option<usize> {
        let name = self.selected.as_ref()?;
        self.rows
            .iter()
            .position(|r| &r.project.metadata.name == name)
    }

    fn move_selection(&mut self, movement: Move, cx: &mut Context<Self>) {
        if let Some(next) = nav::step(self.selected_index(), self.rows.len(), movement) {
            self.selected = Some(self.rows[next].project.metadata.name.clone());
            self.scroll.scroll_to_item(next, ScrollStrategy::Nearest);
            cx.notify();
        }
    }

    fn render_rows(&mut self, range: Range<usize>, cx: &mut Context<Self>) -> Vec<AnyElement> {
        let colors = cx.colors().clone();
        let columns = columns();
        let selected = self.selected_index();
        let now = jiff::Timestamp::now();
        range
            .filter_map(|index| {
                let row = self.rows.get(index)?.clone();
                let spec = &row.project.spec;
                let active: Vec<&SyncWindow> = spec
                    .sync_windows
                    .iter()
                    .filter(|w| windows::is_active(w, now))
                    .collect();
                let windows_cell = if spec.sync_windows.is_empty() {
                    div()
                        .text_color(colors.text_faint)
                        .child("—")
                        .into_any_element()
                } else if active.iter().any(|w| w.kind == "deny") {
                    widgets::pill("deny window now", colors.red).into_any_element()
                } else if active.iter().any(|w| w.kind == "allow") {
                    widgets::pill("allow window now", colors.green).into_any_element()
                } else {
                    widgets::pill(
                        format!(
                            "{} window{}",
                            spec.sync_windows.len(),
                            if spec.sync_windows.len() == 1 {
                                ""
                            } else {
                                "s"
                            }
                        ),
                        colors.text_dim,
                    )
                    .into_any_element()
                };
                let cells: Vec<AnyElement> = vec![
                    div()
                        .truncate()
                        .font_family(fonts::MONO)
                        .text_size(u(12.0))
                        .child(row.project.metadata.name.clone())
                        .into_any_element(),
                    div()
                        .truncate()
                        .text_size(u(12.0))
                        .text_color(colors.text_muted)
                        .child(spec.description.clone().unwrap_or_default())
                        .into_any_element(),
                    widgets::mono(if spec.source_repos.iter().any(|r| r == "*") {
                        "any".into()
                    } else {
                        spec.source_repos.len().to_string()
                    }),
                    widgets::mono(spec.destinations.len().to_string()),
                    widgets::mono(row.apps.to_string()),
                    windows_cell,
                ];
                let name = row.project.metadata.name.clone();
                Some(
                    widgets::row(
                        ("argo-project", index),
                        selected == Some(index),
                        32.0,
                        &colors,
                    )
                    .children(
                        columns
                            .iter()
                            .zip(cells)
                            .map(|(def, cell)| widgets::column_cell(def).child(cell)),
                    )
                    .on_click(cx.listener(move |this, _, window, cx| {
                        this.focus.focus(window, cx);
                        this.selected = Some(name.clone());
                        cx.notify();
                    }))
                    .into_any_element(),
                )
            })
            .collect()
    }

    fn render_details(&self, cx: &mut Context<Self>) -> AnyElement {
        let colors = cx.colors().clone();
        let Some(row) = self
            .selected_index()
            .and_then(|i| self.rows.get(i))
            .cloned()
        else {
            return widgets::empty("Select a project.", &colors);
        };
        let spec = &row.project.spec;
        let list = |title: &str, items: Vec<String>, empty: &str| {
            let mut section = widgets::section(title.to_string(), &colors);
            if items.is_empty() {
                section = section.child(
                    div()
                        .text_size(u(12.0))
                        .text_color(colors.text_dim)
                        .child(empty.to_string()),
                );
            }
            for item in items {
                section = section.child(
                    div()
                        .truncate()
                        .font_family(fonts::MONO)
                        .text_size(u(11.5))
                        .child(item),
                );
            }
            section
        };
        let kinds = |allow: &[GroupKind], deny: &[GroupKind]| {
            let mut out: Vec<String> = allow
                .iter()
                .map(|g| format!("allow {}", g.label()))
                .collect();
            out.extend(deny.iter().map(|g| format!("deny  {}", g.label())));
            out
        };
        let mut roles = widgets::section(format!("Roles · {}", spec.roles.len()), &colors);
        for role in &spec.roles {
            roles = roles.child(
                v_flex()
                    .text_size(u(12.0))
                    .child(
                        h_flex()
                            .gap(u(6.0))
                            .child(
                                div()
                                    .font_weight(FontWeight::MEDIUM)
                                    .child(role.name.clone()),
                            )
                            .when_some(role.description.clone(), |this, d| {
                                this.child(div().text_color(colors.text_dim).child(d))
                            }),
                    )
                    .children(role.policies.iter().map(|p| {
                        div()
                            .pl(u(10.0))
                            .truncate()
                            .font_family(fonts::MONO)
                            .text_size(u(11.0))
                            .text_color(colors.text_muted)
                            .child(p.clone())
                    }))
                    .when(!role.groups.is_empty(), |this| {
                        this.child(
                            div()
                                .pl(u(10.0))
                                .text_color(colors.text_dim)
                                .child(format!("groups: {}", role.groups.join(", "))),
                        )
                    }),
            );
        }
        let mut sync_windows = widgets::section("Sync windows", &colors);
        if spec.sync_windows.is_empty() {
            sync_windows = sync_windows.child(
                div()
                    .text_size(u(12.0))
                    .text_color(colors.text_dim)
                    .child("None: syncs are always allowed."),
            );
        }
        let now = jiff::Timestamp::now();
        let blocking = spec
            .sync_windows
            .iter()
            .any(|w| w.kind == "deny" && windows::is_active(w, now))
            || (spec.sync_windows.iter().any(|w| w.kind == "allow")
                && !spec
                    .sync_windows
                    .iter()
                    .any(|w| w.kind == "allow" && windows::is_active(w, now)));
        if !spec.sync_windows.is_empty() {
            sync_windows = sync_windows.child(
                div()
                    .text_size(u(12.0))
                    .text_color(if blocking { colors.red } else { colors.green })
                    .child(if blocking {
                        "Syncs of matching apps are blocked right now."
                    } else {
                        "Syncs are allowed right now."
                    }),
            );
        }
        for window in &spec.sync_windows {
            sync_windows = sync_windows.child(window_line(window, &colors));
        }
        v_flex()
            .child(
                h_flex()
                    .px(u(14.0))
                    .py(u(10.0))
                    .gap(u(8.0))
                    .border_b_1()
                    .border_color(colors.border_variant)
                    .child(
                        Icon::new(IconName::FolderKanban)
                            .size(14.0)
                            .color(colors.accent),
                    )
                    .child(
                        div()
                            .font_family(fonts::MONO)
                            .font_weight(FontWeight::MEDIUM)
                            .child(row.project.metadata.name.clone()),
                    )
                    .child(Chip::new(format!("{} apps", row.apps))),
            )
            .child(sync_windows)
            .child(list(
                "Source repositories",
                spec.source_repos.clone(),
                "none",
            ))
            .when(!spec.source_namespaces.is_empty(), |this| {
                this.child(list(
                    "Source namespaces (apps in any namespace)",
                    spec.source_namespaces.clone(),
                    "",
                ))
            })
            .child(list(
                "Destinations",
                spec.destinations.iter().map(|d| d.label()).collect(),
                "none",
            ))
            .child(list(
                "Cluster resources",
                kinds(
                    &spec.cluster_resource_whitelist,
                    &spec.cluster_resource_blacklist,
                ),
                "none allowed (the default)",
            ))
            .child(list(
                "Namespace resources",
                kinds(
                    &spec.namespace_resource_whitelist,
                    &spec.namespace_resource_blacklist,
                ),
                "all allowed (the default)",
            ))
            .child(roles)
            .into_any_element()
    }
}

impl Focusable for ProjectsView {
    fn focus_handle(&self, _: &App) -> FocusHandle {
        self.focus.clone()
    }
}

impl TabView for ProjectsView {
    fn tab_title(&self, _: &App) -> SharedString {
        "Projects".into()
    }

    fn tab_icon(&self, _: &App) -> Option<SharedString> {
        Some(IconName::FolderKanban.path())
    }

    fn view_request(&self, _: &App) -> Option<ViewRequest> {
        let gvr = self
            .gvr
            .clone()
            .unwrap_or_else(|| Gvr::new(GROUP, "v1alpha1", "appprojects"));
        Some(ViewRequest::for_resource(
            ViewKind::Custom(VIEW_KIND.into()),
            ResourceRef::list(self.cluster.clone(), gvr, self.namespace.clone()),
        ))
    }
}

impl Render for ProjectsView {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let colors = cx.colors().clone();
        let status = self.projects.as_ref().map(|s| s.read(cx).status().clone());
        let toolbar = h_flex()
            .flex_none()
            .h(u(sizes::TOOLBAR))
            .px(u(12.0))
            .gap(u(6.0))
            .border_b_1()
            .border_color(colors.border_variant)
            .text_color(colors.text_dim)
            .child(Icon::new(IconName::FolderKanban).color(colors.accent))
            .child(
                div()
                    .text_color(colors.text)
                    .font_weight(FontWeight::MEDIUM)
                    .child("Projects"),
            )
            .child("·")
            .child(self.rows.len().to_string());
        let body = if self.rows.is_empty() {
            let message = match status {
                None => "This cluster doesn't serve AppProjects.".to_string(),
                Some(StoreStatus::Forbidden) => "You may not list AppProjects here.".to_string(),
                Some(s) if !s.is_settled() => "Loading…".to_string(),
                _ => "No projects.".to_string(),
            };
            widgets::empty(message, &colors)
        } else {
            uniform_list(
                "argo-projects-rows",
                self.rows.len(),
                cx.processor(|this, range: Range<usize>, _, cx| this.render_rows(range, cx)),
            )
            .flex_1()
            .track_scroll(&self.scroll)
            .into_any_element()
        };
        let details = self.render_details(cx);
        let hints: Vec<(SharedString, SharedString)> = vec![("j/k".into(), "Select".into())];
        v_flex()
            .key_context(CONTEXT)
            .size_full()
            .bg(colors.background)
            .text_color(colors.text)
            .font_family(fonts::UI)
            .text_size(u(sizes::UI_FONT))
            .child(toolbar)
            .child(
                h_flex()
                    .flex_1()
                    .min_h_0()
                    .items_stretch()
                    .child(
                        v_flex()
                            .id("argo-projects-table")
                            .key_context(TABLE)
                            .track_focus(&self.focus)
                            .flex_1()
                            .min_w_0()
                            .on_action(cx.listener(|this, _: &nav::SelectNext, _, cx| {
                                this.move_selection(Move::Next, cx)
                            }))
                            .on_action(cx.listener(|this, _: &nav::SelectPrevious, _, cx| {
                                this.move_selection(Move::Previous, cx)
                            }))
                            .on_action(cx.listener(|this, _: &nav::SelectFirst, _, cx| {
                                this.move_selection(Move::First, cx)
                            }))
                            .on_action(cx.listener(|this, _: &nav::SelectLast, _, cx| {
                                this.move_selection(Move::Last, cx)
                            }))
                            .child(widgets::header(&columns(), &colors))
                            .child(body),
                    )
                    .child(
                        div()
                            .id("argo-project-details")
                            .flex_none()
                            .w(u(380.0))
                            .h_full()
                            .overflow_y_scroll()
                            .border_l_1()
                            .border_color(colors.border)
                            .bg(colors.panel)
                            .child(details),
                    ),
            )
            .child(KeyHints::new(hints))
    }
}
