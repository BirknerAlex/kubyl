//! The Flux overview (board 21): a health banner, counts and ready totals per kind, "Needs
//! attention" (failed, stalled, sources not fetched, waiting for a dependency, suspended),
//! recent activity from the Flux controllers' Events (filter by kind, namespace, warnings) and
//! the controllers with their versions and live readiness (their Deployments are watched).

use std::collections::BTreeSet;
use std::sync::Arc;

use gpui::{
    AnyElement, App, Context, FocusHandle, Focusable, FontWeight, IntoElement, Render,
    SharedString, Subscription, Window, div, prelude::*,
};
use gpui_component::button::{Button as MenuButton, ButtonVariants as _};
use gpui_component::menu::{DropdownMenu as _, PopupMenuItem};
use kubyl_core::actions::OpenView;
use kubyl_core::{ActiveContext, ClusterId, Gvr, ResourceRef, TabView, ViewKind, ViewRequest};
use kubyl_flux_core::detect::{Controller, Install};
use kubyl_flux_core::kinds::{Category, FluxKind};
use kubyl_flux_core::model::FluxObject;
use kubyl_flux_core::overview::{Attention, Health, Summary};
use kubyl_kube::{ConnectionEvent, ConnectionManager};
use kubyl_resources::{ResourceStores, StoreHandle, StoreKey, StoreStatus};
use kubyl_ui::{ActiveColors, Chip, Icon, IconName, fonts, h_flex, sizes, u, v_flex};
use serde_json::Value;

use crate::actions;
use crate::state::{self, Flux};
use crate::widgets;

/// `ViewKind::Custom` of the overview.
pub const VIEW_KIND: &str = "flux_overview";
/// Key context of the overview.
pub const CONTEXT: &str = "FluxOverview";
/// Events listed under "Recent activity".
const ACTIVITY_ROWS: usize = 60;

/// Opens the overview of `cluster`.
pub fn open(cluster: &ClusterId, window: &mut Window, cx: &mut App) {
    window.dispatch_action(
        Box::new(OpenView(ViewRequest::for_resource(
            ViewKind::Custom(VIEW_KIND.into()),
            super::category_ref(cluster, None),
        ))),
        cx,
    );
}

/// The controllers whose Events make up the activity.
const CONTROLLERS: [&str; 6] = [
    "kustomize-controller",
    "helm-controller",
    "source-controller",
    "notification-controller",
    "image-reflector-controller",
    "image-automation-controller",
];

/// What the activity list shows.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct ActivityFilter {
    pub warnings_only: bool,
    pub kind: Option<String>,
    pub namespace: Option<String>,
}

pub struct OverviewView {
    cluster: ClusterId,
    stores: Vec<(FluxKind, StoreHandle)>,
    events: Vec<StoreHandle>,
    /// Deployments in the namespaces the controllers run in, for live readiness.
    controllers: Vec<(String, StoreHandle)>,
    objects: Vec<FluxObject>,
    summary: Summary,
    activity: ActivityFilter,
    focus: FocusHandle,
    parsed: state::ParsedObjects,
    _controller_observers: Vec<Subscription>,
    _observers: Vec<Subscription>,
    _subscriptions: Vec<Subscription>,
}

impl OverviewView {
    pub fn new(cluster: ClusterId, _window: &mut Window, cx: &mut Context<Self>) -> Self {
        let mut subscriptions = Vec::new();
        if let Some(manager) = ConnectionManager::try_global(cx) {
            let id = cluster.clone();
            subscriptions.push(cx.subscribe(&manager, move |this, _, event: &ConnectionEvent, cx| {
                if matches!(event, ConnectionEvent::DiscoveryChanged(c) | ConnectionEvent::StateChanged(c) if *c == id)
                {
                    this.sync_stores(cx);
                }
            }));
        }
        if let Some(flux) = Flux::try_global(cx) {
            subscriptions.push(cx.observe(&flux, |this, _, cx| {
                this.sync_controllers(cx);
                cx.notify();
            }));
        }
        let mut this = Self {
            cluster,
            stores: Vec::new(),
            events: Vec::new(),
            controllers: Vec::new(),
            objects: Vec::new(),
            summary: Summary::default(),
            activity: ActivityFilter::default(),
            focus: cx.focus_handle(),
            parsed: state::ParsedObjects::default(),
            _controller_observers: Vec::new(),
            _observers: Vec::new(),
            _subscriptions: subscriptions,
        };
        this.sync_stores(cx);
        this.sync_controllers(cx);
        this
    }

    /// Watches the Deployments where detection found the controllers.
    fn sync_controllers(&mut self, cx: &mut Context<Self>) {
        let install = Flux::try_global(cx).and_then(|f| f.read(cx).install(&self.cluster));
        let wanted: BTreeSet<String> = install
            .iter()
            .flat_map(|i| i.controllers.iter().map(|c| c.namespace.clone()))
            .collect();
        let current: BTreeSet<String> = self.controllers.iter().map(|(ns, _)| ns.clone()).collect();
        if wanted == current {
            return;
        }
        self.controllers = wanted
            .into_iter()
            .map(|namespace| {
                let handle = ResourceStores::acquire(
                    cx,
                    StoreKey::new(
                        self.cluster.clone(),
                        Gvr::new("apps", "v1", "deployments"),
                        Some(namespace.clone()),
                    ),
                );
                (namespace, handle)
            })
            .collect();
        // Replaced with the watches (old observers go with the old stores).
        self._controller_observers = self
            .controllers
            .iter()
            .map(|(_, handle)| cx.observe(handle.entity(), |_, _, cx| cx.notify()))
            .collect();
    }

    /// The controllers as their Deployments say now (detection's snapshot until the watch
    /// has loaded).
    fn live_controllers(&self, install: &Install, cx: &App) -> Vec<Controller> {
        install
            .controllers
            .iter()
            .map(|controller| {
                let store = self
                    .controllers
                    .iter()
                    .find(|(ns, _)| *ns == controller.namespace)
                    .map(|(_, h)| h.read(cx))
                    .filter(|store| store.status().is_ready());
                match store {
                    Some(store) => controller.with_live(
                        store
                            .get(&kubyl_resources::object_key(
                                Some(&controller.namespace),
                                &controller.name,
                            ))
                            .map(|o| o.as_ref()),
                    ),
                    None => controller.clone(),
                }
            })
            .collect()
    }

    fn sync_stores(&mut self, cx: &mut Context<Self>) {
        let wanted: Vec<(FluxKind, Gvr)> = FluxKind::ALL
            .into_iter()
            .filter_map(|kind| Some((kind, state::gvr(&self.cluster, kind, cx)?)))
            .collect();
        let current: Vec<(FluxKind, Gvr)> = self
            .stores
            .iter()
            .map(|(k, h)| (*k, h.read(cx).key().gvr.clone()))
            .collect();
        if current == wanted && !self.events.is_empty() == !wanted.is_empty() {
            return;
        }
        self.stores = wanted
            .iter()
            .map(|(kind, gvr)| {
                (
                    *kind,
                    ResourceStores::acquire(cx, state::all_key(&self.cluster, gvr)),
                )
            })
            .collect();
        let served: BTreeSet<&str> = wanted.iter().map(|(k, _)| k.controller()).collect();
        self.events = CONTROLLERS
            .into_iter()
            .filter(|c| served.contains(c))
            .map(|controller| {
                ResourceStores::acquire(
                    cx,
                    StoreKey::new(self.cluster.clone(), Gvr::new("", "v1", "events"), None)
                        .fields(format!("source={controller}")),
                )
            })
            .collect();
        // Flux objects change the summary; Events only the activity list (rendered from the
        // stores).
        let mut observers: Vec<Subscription> = self
            .stores
            .iter()
            .map(|(_, h)| cx.observe(h.entity(), |this, _, cx| this.refresh(cx)))
            .collect();
        observers.extend(
            self.events
                .iter()
                .map(|h| cx.observe(h.entity(), |_, _, cx| cx.notify())),
        );
        self._observers = observers;
        self.refresh(cx);
    }

    fn refresh(&mut self, cx: &mut Context<Self>) {
        self.objects = self
            .stores
            .iter()
            .flat_map(|(kind, h)| self.parsed.of(*kind, h.entity(), cx).to_vec())
            .collect();
        self.summary = Summary::new(&self.objects, jiff::Timestamp::now());
        cx.notify();
    }

    /// `Kind namespace/name` of the objects under "Needs attention", in order.
    pub fn attention(&self) -> Vec<(String, Attention)> {
        self.summary
            .problems
            .iter()
            .map(|p| {
                (
                    format!("{} {}", p.object.kind, p.object.key()),
                    p.why.clone(),
                )
            })
            .collect()
    }

    pub fn health(&self) -> Health {
        self.summary.health()
    }

    /// Events of the activity list, newest first, filtered.
    fn activity(&self, cx: &App) -> Vec<Arc<Value>> {
        let mut events: Vec<Arc<Value>> = self
            .events
            .iter()
            .flat_map(|h| h.read(cx).objects().values().cloned().collect::<Vec<_>>())
            .filter(|e| {
                let api_version = e
                    .pointer("/involvedObject/apiVersion")
                    .and_then(Value::as_str)
                    .unwrap_or_default();
                api_version.contains(".toolkit.fluxcd.io/")
            })
            .filter(|e| !self.activity.warnings_only || e["type"].as_str() == Some("Warning"))
            .filter(|e| {
                self.activity.kind.as_deref().is_none_or(|k| {
                    e.pointer("/involvedObject/kind").and_then(Value::as_str) == Some(k)
                })
            })
            .filter(|e| {
                self.activity.namespace.as_deref().is_none_or(|ns| {
                    e.pointer("/involvedObject/namespace")
                        .and_then(Value::as_str)
                        == Some(ns)
                })
            })
            .collect();
        events.sort_by_key(|e| std::cmp::Reverse(kubyl_resources::columns::event_time(e)));
        events.truncate(ACTIVITY_ROWS);
        events
    }

    fn open_object(&self, object: &FluxObject, window: &mut Window, cx: &mut App) {
        if let Some(target) = actions::object_ref(&self.cluster, object, cx) {
            actions::open_object(target, window, cx);
        }
    }

    fn loading(&self, cx: &App) -> bool {
        self.stores.iter().any(|(_, h)| {
            matches!(
                h.read(cx).status(),
                StoreStatus::Waiting | StoreStatus::Loading
            )
        })
    }

    // ----- Rendering -----

    fn render_header(&self, cx: &mut Context<Self>) -> impl IntoElement + use<> {
        let colors = cx.colors().clone();
        let install = Flux::try_global(cx).and_then(|f| f.read(cx).install(&self.cluster));
        let version = install.as_ref().and_then(|i| i.version());
        let namespace = install.as_ref().and_then(|i| i.namespace.clone());
        h_flex()
            .flex_none()
            .h(u(sizes::TOOLBAR))
            .px(u(12.0))
            .gap(u(8.0))
            .border_b_1()
            .border_color(colors.border_variant)
            .child(Icon::new(IconName::Gauge).color(colors.accent))
            .child(div().font_weight(FontWeight::MEDIUM).child("Flux overview"))
            .when_some(version, |this, v| this.child(Chip::new(v).mono()))
            .when_some(namespace, |this, ns| {
                this.child(
                    div()
                        .text_size(u(12.0))
                        .text_color(colors.text_dim)
                        .child(format!("controllers in {ns}")),
                )
            })
            .child(div().flex_1())
            .child(
                Chip::new(if self.loading(cx) { "…" } else { "live" })
                    .dot(colors.green)
                    .text_color(colors.green),
            )
    }

    fn render_banner(&self, cx: &mut Context<Self>) -> AnyElement {
        let colors = cx.colors().clone();
        let health = if self.loading(cx) && self.objects.is_empty() {
            Health::Unknown
        } else {
            self.summary.health()
        };
        let (color, icon, title) = match health {
            Health::Healthy => (colors.green, IconName::CircleCheck, "Healthy"),
            Health::Warning => (colors.yellow, IconName::TriangleAlert, "Needs attention"),
            Health::Failing => (colors.red, IconName::CircleX, "Failing"),
            Health::Unknown => (colors.text_dim, IconName::RefreshCw, "Loading"),
        };
        let install = Flux::try_global(cx).and_then(|f| f.read(cx).install(&self.cluster));
        let controllers = install
            .as_ref()
            .map(|i| {
                let ready = self
                    .live_controllers(i, cx)
                    .iter()
                    .filter(|c| c.is_ready())
                    .count();
                if i.forbidden {
                    "Controllers: not visible (no list on deployments)".to_string()
                } else {
                    format!("Controllers: {ready} of {} ready", i.controllers.len())
                }
            })
            .unwrap_or_else(|| "Controllers: looking…".into());
        h_flex()
            .mx(u(16.0))
            .mt(u(14.0))
            .p(u(12.0))
            .gap(u(12.0))
            .rounded(u(8.0))
            .bg(color.opacity(0.1))
            .border_1()
            .border_color(color.opacity(0.35))
            .child(Icon::new(icon).size(20.0).color(color))
            .child(
                v_flex()
                    .gap(u(2.0))
                    .child(
                        div()
                            .text_size(u(14.0))
                            .font_weight(FontWeight::SEMIBOLD)
                            .text_color(color)
                            .child(title),
                    )
                    .child(
                        div()
                            .text_size(u(12.5))
                            .text_color(colors.text_muted)
                            .child(self.summary.headline()),
                    ),
            )
            .child(div().flex_1())
            .child(
                div()
                    .text_size(u(12.0))
                    .text_color(colors.text_dim)
                    .child(controllers),
            )
            .into_any_element()
    }

    fn render_tiles(&self, cx: &mut Context<Self>) -> AnyElement {
        let colors = cx.colors().clone();
        let caps = Flux::caps(&self.cluster, cx);
        let served = |category: Category| match category {
            Category::Kustomizations => caps.kustomizations,
            Category::HelmReleases => caps.helm_releases,
            Category::Sources => caps.sources,
            Category::ImageAutomation => caps.image_automation,
            Category::Notifications => caps.notifications,
        };
        let tiles = Category::ALL
            .into_iter()
            .filter(|c| served(*c))
            .map(|category| {
                let mut total = 0;
                let mut ready = 0;
                let mut failed = 0;
                let mut suspended = 0;
                for kind in category.kinds() {
                    if let Some(count) = self.summary.counts.get(&kind) {
                        total += count.total;
                        ready += count.ready;
                        failed += count.failed;
                        suspended += count.suspended;
                    }
                }
                let cluster = self.cluster.clone();
                let static_kinds = category == Category::Notifications;
                v_flex()
                    .id(SharedString::from(format!(
                        "flux-tile-{}",
                        category.view_id()
                    )))
                    .flex_1()
                    .min_w(u(120.0))
                    .p(u(12.0))
                    .gap(u(6.0))
                    .rounded(u(8.0))
                    .bg(colors.panel)
                    .border_1()
                    .border_color(colors.border_variant)
                    .cursor_pointer()
                    .hover(|s| s.border_color(colors.border))
                    .on_click(move |_, window, cx| {
                        actions::open_category(&cluster, category, None, window, cx)
                    })
                    .child(
                        h_flex()
                            .gap(u(6.0))
                            .text_size(u(12.0))
                            .text_color(colors.text_dim)
                            .child(Icon::new(widgets::category_icon(category)).size(13.0))
                            .child(category.label()),
                    )
                    .child(
                        h_flex()
                            .items_end()
                            .gap(u(6.0))
                            .child(div().text_size(u(22.0)).font_family(fonts::MONO).child(
                                if static_kinds {
                                    total.to_string()
                                } else {
                                    format!("{ready}/{total}")
                                },
                            ))
                            .child(
                                div()
                                    .pb(u(3.0))
                                    .text_size(u(12.0))
                                    .text_color(colors.text_dim)
                                    .child(if static_kinds { "objects" } else { "ready" }),
                            ),
                    )
                    .child(
                        h_flex()
                            .gap(u(10.0))
                            .text_size(u(11.5))
                            .when(failed > 0, |this| {
                                this.child(widgets::pill(format!("{failed} failing"), colors.red))
                            })
                            .when(suspended > 0, |this| {
                                this.child(widgets::pill(
                                    format!("{suspended} suspended"),
                                    colors.purple,
                                ))
                            })
                            .when(failed == 0 && suspended == 0 && total > 0, |this| {
                                this.child(widgets::pill("all good", colors.green))
                            }),
                    )
            });
        h_flex()
            .mx(u(16.0))
            .mt(u(12.0))
            .gap(u(10.0))
            .children(tiles)
            .into_any_element()
    }

    fn render_attention(&self, cx: &mut Context<Self>) -> AnyElement {
        let colors = cx.colors().clone();
        let mut list = v_flex()
            .flex_1()
            .min_w_0()
            .rounded(u(8.0))
            .border_1()
            .border_color(colors.border_variant)
            .overflow_hidden()
            .child(
                h_flex()
                    .px(u(12.0))
                    .h(u(32.0))
                    .bg(colors.subheader_background)
                    .border_b_1()
                    .border_color(colors.border_variant)
                    .child(widgets::title_line(
                        format!("Needs attention · {}", self.summary.problems.len()),
                        &colors,
                    )),
            );
        if self.summary.problems.is_empty() {
            list = list.child(
                div()
                    .p(u(14.0))
                    .text_size(u(12.0))
                    .text_color(colors.text_dim)
                    .child("Nothing needs attention."),
            );
        }
        for (index, problem) in self.summary.problems.iter().enumerate() {
            let object = problem.object.clone();
            let color = match problem.why {
                Attention::Waiting(_) => colors.yellow,
                Attention::Suspended => colors.purple,
                Attention::Stalled => colors.orange,
                _ => colors.red,
            };
            list =
                list.child(
                    v_flex()
                        .id(("flux-attention", index))
                        .px(u(12.0))
                        .py(u(8.0))
                        .gap(u(3.0))
                        .border_b_1()
                        .border_color(colors.row_border)
                        .cursor_pointer()
                        .hover(|s| s.bg(colors.hover))
                        .on_click(cx.listener(move |this, _, window, cx| {
                            this.open_object(&object, window, cx)
                        }))
                        .child(
                            h_flex()
                                .gap(u(8.0))
                                .text_size(u(12.0))
                                .child(
                                    Icon::new(widgets::kind_icon(problem.object.kind))
                                        .size(13.0)
                                        .color(colors.text_dim),
                                )
                                .child(
                                    div()
                                        .text_color(colors.text_dim)
                                        .child(problem.object.kind.kind()),
                                )
                                .child(
                                    div()
                                        .font_family(fonts::MONO)
                                        .text_size(u(12.0))
                                        .truncate()
                                        .child(problem.object.key()),
                                )
                                .child(div().flex_1())
                                .child(widgets::pill(problem.why.label(), color)),
                        )
                        .child(
                            div()
                                .pl(u(21.0))
                                .text_size(u(12.0))
                                .text_color(colors.text_muted)
                                .line_clamp(2)
                                .child(problem.message.clone()),
                        ),
                );
        }
        list.into_any_element()
    }

    fn render_activity(&self, cx: &mut Context<Self>) -> AnyElement {
        let colors = cx.colors().clone();
        let events = self.activity(cx);
        let weak = cx.entity().downgrade();
        let kinds: Vec<&'static str> = FluxKind::ALL.iter().map(|k| k.kind()).collect();
        let namespaces: Vec<String> = self
            .objects
            .iter()
            .map(|o| o.namespace.clone())
            .collect::<BTreeSet<_>>()
            .into_iter()
            .collect();
        let filter = self.activity.clone();
        let menu = |id: &'static str,
                    label: String,
                    active: bool,
                    choices: Vec<String>,
                    set: fn(&mut ActivityFilter, Option<String>)| {
            let weak = weak.clone();
            MenuButton::new(id)
                .ghost()
                .compact()
                .child(
                    h_flex()
                        .gap(u(4.0))
                        .text_size(u(12.0))
                        .text_color(if active {
                            colors.chip_selected_text
                        } else {
                            colors.text_muted
                        })
                        .child(label)
                        .child(Icon::new(IconName::ChevronDown).size(11.0)),
                )
                .dropdown_menu(move |menu, _, _| {
                    let all = weak.clone();
                    let mut menu = menu.max_h(gpui::px(360.0)).scrollable(true).item(
                        PopupMenuItem::new("All").on_click(move |_, _, cx| {
                            all.update(cx, |this, cx| {
                                set(&mut this.activity, None);
                                cx.notify();
                            })
                            .ok();
                        }),
                    );
                    for choice in &choices {
                        let weak = weak.clone();
                        let value = choice.clone();
                        menu = menu.item(PopupMenuItem::new(choice.clone()).on_click(
                            move |_, _, cx| {
                                let value = value.clone();
                                weak.update(cx, |this, cx| {
                                    set(&mut this.activity, Some(value));
                                    cx.notify();
                                })
                                .ok();
                            },
                        ));
                    }
                    menu
                })
        };
        let kind_menu = menu(
            "flux-activity-kind",
            filter.kind.clone().unwrap_or_else(|| "All kinds".into()),
            filter.kind.is_some(),
            kinds.iter().map(|k| k.to_string()).collect(),
            |f, v| f.kind = v,
        );
        let ns_menu = menu(
            "flux-activity-ns",
            filter
                .namespace
                .clone()
                .unwrap_or_else(|| "All namespaces".into()),
            filter.namespace.is_some(),
            namespaces,
            |f, v| f.namespace = v,
        );
        let warnings = div()
            .id("flux-activity-warnings")
            .cursor_pointer()
            .on_click(cx.listener(|this, _, _, cx| {
                this.activity.warnings_only = !this.activity.warnings_only;
                cx.notify();
            }))
            .child(
                Chip::new("Warnings")
                    .dot(colors.yellow)
                    .selected(filter.warnings_only),
            );
        let now = jiff::Timestamp::now();
        let mut list = v_flex()
            .flex_1()
            .min_w_0()
            .rounded(u(8.0))
            .border_1()
            .border_color(colors.border_variant)
            .overflow_hidden()
            .child(
                h_flex()
                    .px(u(12.0))
                    .h(u(32.0))
                    .gap(u(6.0))
                    .bg(colors.subheader_background)
                    .border_b_1()
                    .border_color(colors.border_variant)
                    .child(widgets::title_line("Recent activity", &colors))
                    .child(div().flex_1())
                    .child(warnings)
                    .child(kind_menu)
                    .child(ns_menu),
            );
        if events.is_empty() {
            list = list.child(
                div()
                    .p(u(14.0))
                    .text_size(u(12.0))
                    .text_color(colors.text_dim)
                    .child("No recent events (Kubernetes keeps them for an hour)."),
            );
        }
        for (index, event) in events.iter().enumerate() {
            let warning = event["type"].as_str() == Some("Warning");
            let kind = event
                .pointer("/involvedObject/kind")
                .and_then(Value::as_str)
                .unwrap_or_default()
                .to_string();
            let name = format!(
                "{}/{}",
                event
                    .pointer("/involvedObject/namespace")
                    .and_then(Value::as_str)
                    .unwrap_or_default(),
                event
                    .pointer("/involvedObject/name")
                    .and_then(Value::as_str)
                    .unwrap_or_default()
            );
            let age = kubyl_resources::columns::event_time(event)
                .map(|t| {
                    kubyl_resources::format::human_duration(kubyl_resources::format::seconds_since(
                        t, now,
                    ))
                })
                .unwrap_or_default();
            list =
                list.child(
                    h_flex()
                        .id(("flux-event", index))
                        .px(u(12.0))
                        .py(u(6.0))
                        .gap(u(8.0))
                        .items_start()
                        .border_b_1()
                        .border_color(colors.row_border)
                        .text_size(u(12.0))
                        .child(
                            div()
                                .flex_none()
                                .w(u(36.0))
                                .font_family(fonts::MONO)
                                .text_size(u(11.5))
                                .text_color(colors.text_dim)
                                .child(age),
                        )
                        .child(
                            Icon::new(if warning {
                                IconName::TriangleAlert
                            } else {
                                IconName::Info
                            })
                            .size(13.0)
                            .color(if warning {
                                colors.yellow
                            } else {
                                colors.text_dim
                            }),
                        )
                        .child(
                            v_flex()
                                .min_w_0()
                                .flex_1()
                                .child(
                                    h_flex()
                                        .gap(u(6.0))
                                        .child(div().text_color(colors.text_dim).child(kind))
                                        .child(
                                            div()
                                                .font_family(fonts::MONO)
                                                .text_size(u(11.5))
                                                .truncate()
                                                .child(name),
                                        )
                                        .child(
                                            div()
                                                .text_color(if warning {
                                                    colors.yellow
                                                } else {
                                                    colors.text_muted
                                                })
                                                .child(
                                                    event["reason"]
                                                        .as_str()
                                                        .unwrap_or_default()
                                                        .to_string(),
                                                ),
                                        ),
                                )
                                .child(div().text_color(colors.text_muted).line_clamp(2).child(
                                    kubyl_resources::columns::event_message(event).to_string(),
                                )),
                        ),
                );
        }
        list.into_any_element()
    }

    fn render_controllers(&self, cx: &mut Context<Self>) -> AnyElement {
        let colors = cx.colors().clone();
        let install = Flux::try_global(cx).and_then(|f| f.read(cx).install(&self.cluster));
        let mut section = v_flex()
            .mx(u(16.0))
            .mb(u(16.0))
            .rounded(u(8.0))
            .border_1()
            .border_color(colors.border_variant)
            .overflow_hidden()
            .child(
                h_flex()
                    .px(u(12.0))
                    .h(u(32.0))
                    .bg(colors.subheader_background)
                    .border_b_1()
                    .border_color(colors.border_variant)
                    .child(widgets::title_line("Controllers", &colors)),
            );
        let Some(install) = install else {
            return section
                .child(
                    div()
                        .p(u(12.0))
                        .text_size(u(12.0))
                        .text_color(colors.text_dim)
                        .child("Looking for Flux's controllers…"),
                )
                .into_any_element();
        };
        if install.forbidden {
            section = section.child(
                div()
                    .p(u(12.0))
                    .text_size(u(12.0))
                    .text_color(colors.text_dim)
                    .child(format!(
                        "You may not list Deployments in {}: versions and controller logs need list on deployments.apps there.",
                        install.namespace.as_deref().unwrap_or("flux-system")
                    )),
            );
        }
        for controller in &self.live_controllers(&install, cx) {
            section = section.child(
                h_flex()
                    .px(u(12.0))
                    .h(u(28.0))
                    .gap(u(10.0))
                    .text_size(u(12.0))
                    .border_b_1()
                    .border_color(colors.row_border)
                    .child(widgets::pill(
                        "",
                        if controller.is_ready() {
                            colors.green
                        } else {
                            colors.red
                        },
                    ))
                    .child(
                        div()
                            .w(u(220.0))
                            .font_family(fonts::MONO)
                            .child(controller.name.clone()),
                    )
                    .child(
                        div()
                            .w(u(80.0))
                            .font_family(fonts::MONO)
                            .text_color(colors.text_muted)
                            .child(controller.version.clone().unwrap_or_default()),
                    )
                    .child(div().text_color(colors.text_dim).child(format!(
                        "{}/{} ready",
                        controller.ready, controller.replicas
                    ))),
            );
        }
        section.into_any_element()
    }
}

impl Focusable for OverviewView {
    fn focus_handle(&self, _: &App) -> FocusHandle {
        self.focus.clone()
    }
}

impl TabView for OverviewView {
    fn tab_title(&self, _: &App) -> SharedString {
        "Flux".into()
    }

    fn tab_icon(&self, _: &App) -> Option<SharedString> {
        Some(IconName::Gauge.path())
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
            ResourceRef::list(self.cluster.clone(), Gvr::new("", "", ""), None),
        ))
    }
}

impl Render for OverviewView {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let colors = cx.colors().clone();
        let header = self.render_header(cx);
        let banner = self.render_banner(cx);
        let tiles = self.render_tiles(cx);
        let attention = self.render_attention(cx);
        let activity = self.render_activity(cx);
        let controllers = self.render_controllers(cx);
        let body: AnyElement = if Flux::caps(&self.cluster, cx).any() || !self.objects.is_empty() {
            v_flex()
                .id("flux-overview-body")
                .flex_1()
                .min_h_0()
                .overflow_y_scroll()
                .child(banner)
                .child(tiles)
                .child(
                    h_flex()
                        .mx(u(16.0))
                        .my(u(12.0))
                        .gap(u(12.0))
                        .items_start()
                        .child(attention)
                        .child(activity),
                )
                .child(controllers)
                .into_any_element()
        } else {
            widgets::empty("This cluster doesn't serve Flux's CRDs.", &colors)
        };
        v_flex()
            .key_context(CONTEXT)
            .track_focus(&self.focus)
            .size_full()
            .bg(colors.background)
            .text_color(colors.text)
            .font_family(fonts::UI)
            .child(header)
            .child(body)
    }
}
