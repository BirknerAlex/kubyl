//! Helm releases: the list, a release's summary, history and resources. Values, manifest and
//! notes open in the release's own tab ([`crate::release`]).

use std::ops::Range;

use gpui::{
    AnyElement, App, Context, FontWeight, IntoElement, Task, Window, div, prelude::*, px,
    uniform_list,
};
use gpui_component::button::{Button as MenuButton, ButtonVariants as _};
use gpui_component::menu::{DropdownMenu as _, PopupMenuItem};
use kubyl_core::actions::OpenView;
use kubyl_core::{
    ClusterId, ColumnDef, ColumnWidth, Gvk, ResourceRef, Tone, ViewRegistry, ViewRequest,
    spawn_kube,
};
use kubyl_kube::ConnectionManager;
use kubyl_ui::{ActiveColors, Button, Chip, Colors, Icon, IconName, fonts, h_flex, u, v_flex};

use super::{OperatorsView, SubTab};
use crate::helm::present::{self, ManifestObject};
use crate::helm::service::{Helm, ReleaseRow, SummaryState};
use crate::release::ReleaseTab;
use crate::widgets;

const ROW_HEIGHT: f32 = 32.0;

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

/// The kubeconfig context name of a cluster, for `helm --kube-context`.
pub fn context_name(cluster: &ClusterId, cx: &App) -> Option<String> {
    Some(
        ConnectionManager::try_global(cx)?
            .read(cx)
            .context(cluster)?
            .context
            .clone(),
    )
}

impl OperatorsView {
    fn helm_snapshot(&self, cx: &App) -> Option<std::sync::Arc<crate::helm::service::Snapshot>> {
        Helm::global(cx)?.read(cx).snapshot(&self.cluster, cx)
    }

    pub(crate) fn selected_release(&self, cx: &App) -> Option<ReleaseRow> {
        let key = self.selected.get(&SubTab::Helm)?;
        self.helm_snapshot(cx)?
            .releases
            .iter()
            .find(|r| &release_key(r) == key)
            .cloned()
    }

    pub(crate) fn helm_summary(&self, cx: &App) -> String {
        let Some(snapshot) = self.helm_snapshot(cx) else {
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

    fn helm_columns(&self) -> Vec<ColumnDef> {
        let details = self.details_open && self.selected_key().is_some();
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

    pub(crate) fn render_helm(&mut self, _: &mut Window, cx: &mut Context<Self>) -> AnyElement {
        let colors = cx.colors().clone();
        let Some(snapshot) = self.helm_snapshot(cx) else {
            return widgets::empty(format!("Connecting to {}…", self.cluster_name(cx)), &colors);
        };
        let query = self.query(cx);
        let namespace = self.helm_namespace.clone();
        self.helm_rows = snapshot
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
        self.helm_rows
            .sort_by(|a, b| (&a.name, &a.namespace).cmp(&(&b.name, &b.namespace)));
        self.keys = self.helm_rows.iter().map(release_key).collect();
        if self.selected_key().is_none()
            && let Some(first) = self.keys.first().cloned()
        {
            self.selected.insert(SubTab::Helm, first);
        }
        let mut banners: Vec<AnyElement> = Vec::new();
        if let Some(problem) = &snapshot.problem {
            banners.push(self.render_helm_problem(problem.clone(), snapshot.scope.clone(), cx));
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
                                this.helm_namespace = None;
                                cx.notify();
                            })),
                    )
                    .into_any_element(),
            );
        }
        let body = if self.helm_rows.is_empty() {
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
            let columns = self.helm_columns();
            v_flex()
                .flex_1()
                .min_h_0()
                .child(widgets::header(&columns, &colors))
                .child(
                    uniform_list(
                        "helm-rows",
                        self.helm_rows.len(),
                        cx.processor(|this, range: Range<usize>, _, cx| {
                            this.render_helm_rows(range, cx)
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
            .map(|row| self.render_release_details(row, cx));
        v_flex()
            .size_full()
            .children(banners)
            .child(
                self.focus_area()
                    .items_start()
                    .child(v_flex().flex_1().min_w_0().h_full().child(body))
                    .children(details),
            )
            .into_any_element()
    }

    fn render_helm_problem(
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

    fn render_helm_rows(&mut self, range: Range<usize>, cx: &mut Context<Self>) -> Vec<AnyElement> {
        let colors = cx.colors().clone();
        let columns = self.helm_columns();
        let selected = self.selected_key().cloned();
        range
            .filter_map(|index| {
                let row = self.helm_rows.get(index)?.clone();
                let key = release_key(&row);
                Some(
                    widgets::row(
                        ("helm-row", index),
                        selected.as_ref() == Some(&key),
                        ROW_HEIGHT,
                        &colors,
                    )
                    .children(columns.iter().map(|def| {
                        widgets::column_cell(def).child(helm_cell(&row, def.id.as_ref(), &colors))
                    }))
                    .on_click(
                        cx.listener(move |this, event: &gpui::ClickEvent, window, cx| {
                            this.focus.focus(window, cx);
                            this.select(key.clone(), cx);
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
        if self.helm_resources.as_ref().is_some_and(|(k, _)| k == &key) {
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
                let release = crate::helm::service::load(client, driver, namespace, object).await?;
                Ok::<_, String>(present::manifest_objects(&release.manifest))
            }
        });
        let task_key = key.clone();
        let task = cx.spawn(async move |this, cx| {
            let result = load.await;
            this.update(cx, |this, cx| {
                if this
                    .helm_resources
                    .as_ref()
                    .is_some_and(|(k, _)| k == &task_key)
                {
                    this.helm_resources = Some((
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
        self.helm_resources = Some((key, HelmResources::Loading(task)));
    }

    fn render_release_details(&mut self, row: ReleaseRow, cx: &mut Context<Self>) -> AnyElement {
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
                        widgets::mono(format!("{} {}", s.chart.name, s.chart.version)),
                    ),
                    (
                        "App version",
                        widgets::mono(s.chart.app_version.clone().unwrap_or_else(|| "—".into())),
                    ),
                ];
                if let Some(first) = s.first_deployed {
                    rows.push((
                        "First deployed",
                        widgets::text(format!("{} ago", widgets::ago(Some(first)))),
                    ));
                }
                if let Some(description) = &s.description {
                    rows.push(("Description", widgets::text(description.clone())));
                }
                rows.push((
                    "Stored in",
                    widgets::text(format!("{}s · {}", row.driver.label(), latest.object)),
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
        summary = summary.child(
            h_flex()
                .pt(u(4.0))
                .gap(u(6.0))
                .child(
                    Button::new("release-open")
                        .primary()
                        .icon(IconName::Anchor)
                        .label("Open release")
                        .on_click(
                            cx.listener(|this, _, window, cx| this.open_release(None, window, cx)),
                        ),
                )
                .child(
                    Button::new("release-commands")
                        .icon(IconName::Copy)
                        .label("Copy helm command…")
                        .on_click(
                            cx.listener(|this, _, window, cx| this.helm_commands(window, cx)),
                        ),
                ),
        );
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
        let resources = match self.helm_resources.as_ref().map(|(_, r)| r) {
            Some(HelmResources::Ready(objects)) => {
                let own: Vec<&ManifestObject> = objects.iter().filter(|o| !o.hook).collect();
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

    pub(crate) fn open_release(
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

    pub(crate) fn helm_commands(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Some(row) = self.selected_release(cx) else {
            return;
        };
        let latest = row.latest().revision;
        let commands = present::commands(
            &row.name,
            &row.namespace,
            latest,
            latest,
            context_name(&self.cluster, cx).as_deref(),
        );
        crate::dialogs::open_helm_commands(
            format!("helm commands for {}", row.name),
            commands,
            window,
            cx,
        );
    }
}

fn helm_cell(row: &ReleaseRow, column: &str, colors: &Colors) -> AnyElement {
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
