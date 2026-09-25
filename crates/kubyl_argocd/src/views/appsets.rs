//! ApplicationSets: generators in short, sync policy, conditions, and the Applications each one
//! generated (by owner reference) with their health.

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
use kubyl_ui::{ActiveColors, Chip, Icon, IconName, KeyHints, fonts, h_flex, sizes, u, v_flex};

use crate::actions;
use crate::model::{Application, ApplicationSet, GROUP, generator_summary};
use crate::nav::{self, Move, TABLE};
use crate::state;
use crate::widgets;

pub const VIEW_KIND: &str = "argocd_appsets";
const CONTEXT: &str = "ArgoSets";

fn columns() -> Vec<ColumnDef> {
    vec![
        ColumnDef::new(
            "name",
            "Name",
            ColumnWidth::Flex {
                weight: 1.0,
                min: 160.0,
            },
        ),
        ColumnDef::new(
            "generators",
            "Generators",
            ColumnWidth::Flex {
                weight: 1.4,
                min: 180.0,
            },
        ),
        ColumnDef::new("policy", "Policy", ColumnWidth::Fixed(130.0)),
        ColumnDef::new("apps", "Apps", ColumnWidth::Fixed(90.0)),
        ColumnDef::new("status", "Status", ColumnWidth::Fixed(150.0)),
        ColumnDef::new("age", "Age", ColumnWidth::Fixed(56.0)),
    ]
}

struct Row {
    set: ApplicationSet,
    uid: String,
    /// Generated apps.
    apps: Vec<Application>,
}

pub struct AppSetsView {
    cluster: ClusterId,
    namespace: Option<String>,
    gvr: Option<Gvr>,
    sets: Option<StoreHandle>,
    apps: Option<StoreHandle>,
    rows: Vec<Arc<Row>>,
    selected: Option<String>,
    focus: FocusHandle,
    scroll: UniformListScrollHandle,
    _observers: Vec<Subscription>,
}

impl AppSetsView {
    pub fn new(target: ResourceRef, _: &mut Window, cx: &mut Context<Self>) -> Self {
        let mut this = Self {
            cluster: target.cluster.clone(),
            namespace: target.namespace.clone(),
            gvr: None,
            sets: None,
            apps: None,
            rows: Vec::new(),
            selected: None,
            focus: cx.focus_handle(),
            scroll: UniformListScrollHandle::new(),
            _observers: Vec::new(),
        };
        if let Some(manager) = kubyl_kube::ConnectionManager::try_global(cx) {
            let cluster = this.cluster.clone();
            this._observers.push(cx.subscribe(&manager, move |this, _, event: &kubyl_kube::ConnectionEvent, cx| {
                if matches!(event, kubyl_kube::ConnectionEvent::DiscoveryChanged(id) if *id == cluster) {
                    this.sync_stores(cx);
                }
            }));
        }
        this.sync_stores(cx);
        this
    }

    fn sync_stores(&mut self, cx: &mut Context<Self>) {
        let gvr = state::resource(&self.cluster, "applicationsets", cx).map(|(g, _)| g);
        if gvr == self.gvr && self.sets.is_some() {
            return;
        }
        self.gvr = gvr.clone();
        self.sets = gvr.map(|gvr| {
            ResourceStores::acquire(
                cx,
                StoreKey::new(self.cluster.clone(), gvr, self.namespace.clone()),
            )
        });
        self.apps = state::resource(&self.cluster, "applications", cx).map(|(gvr, _)| {
            ResourceStores::acquire(
                cx,
                StoreKey::new(self.cluster.clone(), gvr, self.namespace.clone()),
            )
        });
        let mut observers: Vec<Subscription> = [&self.sets, &self.apps]
            .into_iter()
            .flatten()
            .map(|h| cx.observe(h.entity(), |this, _, cx| this.refresh(cx)))
            .collect();
        observers.extend(self._observers.drain(..1.min(self._observers.len())));
        self._observers = observers;
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
            .sets
            .as_ref()
            .map(|h| {
                h.read(cx)
                    .objects()
                    .values()
                    .filter_map(|o| {
                        let set = ApplicationSet::parse(o)?;
                        let uid = set.metadata.uid.clone();
                        let generated = apps
                            .iter()
                            .filter(|a| {
                                a.metadata.owner_references.iter().any(|r| {
                                    r.kind == "ApplicationSet"
                                        && (r.uid == uid
                                            || (uid.is_empty() && r.name == set.metadata.name))
                                })
                            })
                            .cloned()
                            .collect();
                        Some(Arc::new(Row {
                            set,
                            uid,
                            apps: generated,
                        }))
                    })
                    .collect()
            })
            .unwrap_or_default();
        rows.sort_by(|a, b| a.set.metadata.name.cmp(&b.set.metadata.name));
        self.rows = rows;
        if self.selected.is_none() {
            self.selected = self.rows.first().map(|r| r.uid.clone());
        }
        cx.notify();
    }

    fn selected_index(&self) -> Option<usize> {
        let uid = self.selected.as_ref()?;
        self.rows.iter().position(|r| &r.uid == uid)
    }

    fn move_selection(&mut self, movement: Move, cx: &mut Context<Self>) {
        if let Some(next) = nav::step(self.selected_index(), self.rows.len(), movement) {
            self.selected = Some(self.rows[next].uid.clone());
            self.scroll.scroll_to_item(next, ScrollStrategy::Nearest);
            cx.notify();
        }
    }

    fn render_rows(&mut self, range: Range<usize>, cx: &mut Context<Self>) -> Vec<AnyElement> {
        let colors = cx.colors().clone();
        let columns = columns();
        let selected = self.selected_index();
        range
            .filter_map(|index| {
                let row = self.rows.get(index)?.clone();
                let set = &row.set;
                let problems: Vec<_> = set
                    .status
                    .conditions
                    .iter()
                    .filter(|c| c.is_problem())
                    .collect();
                let status = match problems.first() {
                    Some(problem) => {
                        widgets::pill(problem.kind.clone(), colors.red).into_any_element()
                    }
                    None => widgets::pill("OK", colors.green).into_any_element(),
                };
                let degraded = row
                    .apps
                    .iter()
                    .filter(|a| a.health() == crate::model::Health::Degraded)
                    .count();
                let apps = h_flex()
                    .gap(u(6.0))
                    .child(widgets::mono(row.apps.len().to_string()))
                    .when(degraded > 0, |this| {
                        this.child(
                            div()
                                .text_size(u(11.5))
                                .text_color(colors.red)
                                .child(format!("{degraded} degraded")),
                        )
                    })
                    .into_any_element();
                let cells: Vec<AnyElement> = vec![
                    div()
                        .truncate()
                        .font_family(fonts::MONO)
                        .text_size(u(12.0))
                        .child(set.metadata.name.clone())
                        .into_any_element(),
                    div()
                        .truncate()
                        .text_size(u(12.0))
                        .child(set.generators_summary())
                        .into_any_element(),
                    div()
                        .truncate()
                        .text_size(u(12.0))
                        .text_color(colors.text_muted)
                        .child(set.policy_summary())
                        .into_any_element(),
                    apps,
                    status,
                    widgets::mono(widgets::age(set.metadata.creation_timestamp.as_deref())),
                ];
                let uid = row.uid.clone();
                Some(
                    widgets::row(("argo-set", index), selected == Some(index), 32.0, &colors)
                        .children(
                            columns
                                .iter()
                                .zip(cells)
                                .map(|(def, cell)| widgets::column_cell(def).child(cell)),
                        )
                        .on_click(cx.listener(move |this, _, window, cx| {
                            this.focus.focus(window, cx);
                            this.selected = Some(uid.clone());
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
            return widgets::empty("Select an ApplicationSet.", &colors);
        };
        let set = &row.set;
        let mut generators = widgets::section("Generators", &colors);
        for generator in &set.spec.generators {
            generators = generators.child(
                h_flex()
                    .gap(u(8.0))
                    .text_size(u(12.0))
                    .child(
                        Icon::new(IconName::GitFork)
                            .size(13.0)
                            .color(colors.text_dim),
                    )
                    .child(div().flex_1().min_w_0().child(generator_summary(generator))),
            );
        }
        let mut facts: Vec<(&'static str, AnyElement)> = vec![
            ("Policy", widgets::text(set.policy_summary())),
            (
                "Templating",
                widgets::text(if set.spec.go_template {
                    "Go templates"
                } else {
                    "fasttemplate"
                }),
            ),
        ];
        if set.rolling_sync() {
            facts.push(("Strategy", widgets::text("RollingSync (progressive)")));
        }
        let policy = widgets::section("Sync policy", &colors).child(widgets::kv(facts, &colors));
        let mut conditions = widgets::section("Conditions", &colors);
        for condition in &set.status.conditions {
            let color = if condition.is_problem() {
                colors.red
            } else {
                colors.green
            };
            conditions = conditions.child(
                v_flex()
                    .text_size(u(12.0))
                    .child(h_flex().gap(u(6.0)).child(widgets::pill(
                        format!("{} {}", condition.kind, condition.status),
                        color,
                    )))
                    .when(!condition.message.is_empty(), |this| {
                        this.child(
                            div()
                                .pl(u(13.0))
                                .text_color(colors.text_dim)
                                .child(condition.message.clone()),
                        )
                    }),
            );
        }
        let mut apps = widgets::section(format!("Applications · {}", row.apps.len()), &colors);
        let gvr = state::resource(&self.cluster, "applications", cx).map(|(g, _)| g);
        for (index, app) in row.apps.iter().enumerate() {
            let target = gvr.clone().map(|gvr| {
                ResourceRef::object(
                    self.cluster.clone(),
                    gvr,
                    Some(app.namespace().to_string()),
                    app.name().to_string(),
                )
            });
            apps = apps.child(
                h_flex()
                    .gap(u(8.0))
                    .text_size(u(12.0))
                    .child(match target {
                        Some(target) => widgets::link(
                            ("argo-set-app", index),
                            app.name().to_string(),
                            &colors,
                            move |_, window, cx| {
                                actions::open_app(target.clone(), None, window, cx)
                            },
                        )
                        .into_any_element(),
                        None => widgets::mono(app.name().to_string()),
                    })
                    .child(div().flex_1())
                    .child(widgets::sync_pill(app.sync(), &colors))
                    .child(widgets::health_pill(app.health(), &colors)),
            );
        }
        if row.apps.is_empty() {
            apps = apps.child(
                div()
                    .text_size(u(12.0))
                    .text_color(colors.text_dim)
                    .child("No Applications generated."),
            );
        }
        v_flex()
            .child(
                h_flex()
                    .px(u(14.0))
                    .py(u(10.0))
                    .gap(u(8.0))
                    .border_b_1()
                    .border_color(colors.border_variant)
                    .child(Icon::new(IconName::GitFork).size(14.0).color(colors.accent))
                    .child(
                        div()
                            .font_family(fonts::MONO)
                            .font_weight(FontWeight::MEDIUM)
                            .child(set.metadata.name.clone()),
                    )
                    .child(Chip::new(
                        set.metadata.namespace.clone().unwrap_or_default(),
                    )),
            )
            .child(generators)
            .child(policy)
            .child(conditions)
            .child(apps)
            .into_any_element()
    }
}

impl Focusable for AppSetsView {
    fn focus_handle(&self, _: &App) -> FocusHandle {
        self.focus.clone()
    }
}

impl TabView for AppSetsView {
    fn tab_title(&self, _: &App) -> SharedString {
        "ApplicationSets".into()
    }

    fn tab_icon(&self, _: &App) -> Option<SharedString> {
        Some(IconName::GitFork.path())
    }

    fn view_request(&self, _: &App) -> Option<ViewRequest> {
        let gvr = self
            .gvr
            .clone()
            .unwrap_or_else(|| Gvr::new(GROUP, "v1alpha1", "applicationsets"));
        Some(ViewRequest::for_resource(
            ViewKind::Custom(VIEW_KIND.into()),
            ResourceRef::list(self.cluster.clone(), gvr, self.namespace.clone()),
        ))
    }
}

impl Render for AppSetsView {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let colors = cx.colors().clone();
        let status = self.sets.as_ref().map(|s| s.read(cx).status().clone());
        let problems = self
            .rows
            .iter()
            .filter(|r| r.set.status.conditions.iter().any(|c| c.is_problem()))
            .count();
        let toolbar = h_flex()
            .flex_none()
            .h(u(sizes::TOOLBAR))
            .px(u(12.0))
            .gap(u(6.0))
            .border_b_1()
            .border_color(colors.border_variant)
            .text_color(colors.text_dim)
            .child(Icon::new(IconName::GitFork).color(colors.accent))
            .child(
                div()
                    .text_color(colors.text)
                    .font_weight(FontWeight::MEDIUM)
                    .child("ApplicationSets"),
            )
            .child("·")
            .child(self.rows.len().to_string())
            .when(problems > 0, |this| {
                this.child(
                    div()
                        .text_color(colors.red)
                        .child(format!("· {problems} with problems")),
                )
            });
        let body = if self.rows.is_empty() {
            let message = match status {
                None => "This cluster doesn't serve ApplicationSets.".to_string(),
                Some(StoreStatus::Forbidden) => {
                    "You may not list ApplicationSets here.".to_string()
                }
                Some(s) if !s.is_settled() => "Loading…".to_string(),
                _ => "No ApplicationSets.".to_string(),
            };
            widgets::empty(message, &colors)
        } else {
            uniform_list(
                "argo-sets-rows",
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
                            .id("argo-sets-table")
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
                            .id("argo-set-details")
                            .flex_none()
                            .w(u(360.0))
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
