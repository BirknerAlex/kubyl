//! Argo CD in other crates' views:
//! - the details dock of an Application: status, actions, what's out of sync, source, sync
//!   policy and the last operation (board 12's dock);
//! - "Managed by Argo CD app X" in the details of objects Argo CD tracks, with a link;
//! - the YAML editor's warning that Argo CD may revert edits (self-heal).
//!
//! Tracking: the `argocd.argoproj.io/tracking-id` annotation (Argo CD 3.x default) names the app
//! and the object; it counts only when it matches the object (a copied manifest keeps the
//! annotation but isn't managed). The `app.kubernetes.io/instance` label (label tracking) is also
//! what Helm sets, so it counts only when an Application of that name exists.

use std::sync::Arc;

use gpui::{
    AnyView, App, AppContext as _, Context, Entity, IntoElement, Render, SharedString,
    Subscription, Window, div, prelude::*,
};
use kubyl_core::{ClusterId, DetailsSection, EditNotice, ResourceRef};
use kubyl_resources::{
    ResourceSelection, ResourceStore, ResourceStores, StoreHandle, StoreKey, object_key,
};
use kubyl_ui::{ActiveColors, Button, IconName, fonts, h_flex, u, v_flex};
use serde_json::Value;

use crate::actions;
use crate::dialogs;
use crate::model::{Application, GROUP, SyncStatus};
use crate::ops::PolicyChange;
use crate::run::{self, Op};
use crate::state::{self, ArgoCd};
use crate::widgets;

pub use kubyl_argocd_core::tracking::{ManagedBy, managed_by};

/// The Applications watch of a cluster (all namespaces), if some view runs it.
fn loaded_apps(cluster: &ClusterId, cx: &App) -> Vec<Application> {
    let Some((gvr, _)) = state::resource(cluster, "applications", cx) else {
        return Vec::new();
    };
    ResourceStores::peek(cx, &crate::apps::all_key(cluster, &gvr))
        .map(|store| {
            store
                .read(cx)
                .objects()
                .values()
                .filter_map(|o| Application::parse(o))
                .collect()
        })
        .unwrap_or_default()
}

/// The namespace an app of `managed` lives in: its own, else Argo CD's.
fn app_namespace(cluster: &ClusterId, managed: &ManagedBy, cx: &App) -> String {
    managed.app_namespace.clone().unwrap_or_else(|| {
        ArgoCd::try_global(cx)
            .and_then(|a| a.read(cx).install_for(cluster, None))
            .map(|i| i.namespace)
            .unwrap_or_else(|| "argocd".into())
    })
}

// ----- Edit notice -----

pub struct ArgoNotice;

impl EditNotice for ArgoNotice {
    fn notice(&self, target: &ResourceRef, object: &Value, cx: &App) -> Option<SharedString> {
        if !ArgoCd::caps(&target.cluster, cx).applications {
            return None;
        }
        let managed = managed_by(object)?;
        let apps = loaded_apps(&target.cluster, cx);
        let app = apps.iter().find(|a| {
            a.name() == managed.app_name
                && managed
                    .app_namespace
                    .as_deref()
                    .is_none_or(|ns| a.namespace() == ns)
        });
        if managed.via_label && app.is_none() {
            return None;
        }
        let name = &managed.app_name;
        Some(match app.map(|a| a.policy()) {
            Some(policy) if policy.self_heal() => format!(
                "Managed by Argo CD app {name} with self-heal: Argo CD reverts changes made here."
            ),
            Some(policy) if policy.auto_sync() => format!(
                "Managed by Argo CD app {name}: changes made here show as out of sync; the next sync reverts them."
            ),
            Some(_) => format!(
                "Managed by Argo CD app {name}: changes made here show as out of sync until the app is synced (which reverts them)."
            ),
            None => format!("Managed by Argo CD app {name}: Argo CD may revert changes made here (self-heal)."),
        }
        .into())
    }
}

// ----- Details sections -----

pub struct ArgoDetails;

impl DetailsSection for ArgoDetails {
    fn id(&self) -> &'static str {
        "argocd"
    }

    fn order(&self) -> i32 {
        -10
    }

    fn build(&self, target: &ResourceRef, kind: &str, cx: &mut App) -> Option<AnyView> {
        let caps = ArgoCd::caps(&target.cluster, cx);
        if !caps.applications {
            return None;
        }
        let target = target.clone();
        if kind == "Application" && target.gvr.group == GROUP {
            return Some(cx.new(|cx| AppDock::new(target, cx)).into());
        }
        Some(cx.new(|cx| ManagedSection::new(target, cx)).into())
    }
}

/// The object's store: the list's (from the selection) or a watch of its own.
fn object_store(
    target: &ResourceRef,
    cx: &mut App,
) -> (Entity<ResourceStore>, Option<StoreHandle>) {
    let selection = ResourceSelection::global(cx);
    if let Some(primary) = selection.primary()
        && primary.target == *target
        && let Some(store) = primary.store.clone()
    {
        return (store, None);
    }
    let key = StoreKey::new(
        target.cluster.clone(),
        target.gvr.clone(),
        target.namespace.clone(),
    )
    .fields(format!(
        "metadata.name={}",
        target.name.as_deref().unwrap_or_default()
    ));
    let handle = ResourceStores::acquire(cx, key);
    (handle.entity().clone(), Some(handle))
}

fn object_of(target: &ResourceRef, store: &Entity<ResourceStore>, cx: &App) -> Option<Arc<Value>> {
    let key = object_key(target.namespace.as_deref(), target.name.as_deref()?);
    store.read(cx).get(&key).cloned()
}

/// "Managed by Argo CD app X".
struct ManagedSection {
    target: ResourceRef,
    store: Entity<ResourceStore>,
    _own: Option<StoreHandle>,
    /// The app's watch, once the object names it.
    app: Option<(ResourceRef, StoreHandle)>,
    /// All apps (label tracking needs to know the app exists).
    all_apps: Option<StoreHandle>,
    _subscriptions: Vec<Subscription>,
}

impl ManagedSection {
    fn new(target: ResourceRef, cx: &mut Context<Self>) -> Self {
        let (store, own) = object_store(&target, cx);
        let subscriptions = vec![cx.observe(&store, |this, _, cx| {
            this.sync(cx);
            cx.notify();
        })];
        let mut this = Self {
            target,
            store,
            _own: own,
            app: None,
            all_apps: None,
            _subscriptions: subscriptions,
        };
        this.sync(cx);
        this
    }

    fn sync(&mut self, cx: &mut Context<Self>) {
        let Some(managed) = object_of(&self.target, &self.store, cx).and_then(|o| managed_by(&o))
        else {
            return;
        };
        let Some((gvr, _)) = state::resource(&self.target.cluster, "applications", cx) else {
            return;
        };
        if managed.via_label && self.all_apps.is_none() {
            let handle =
                ResourceStores::acquire(cx, crate::apps::all_key(&self.target.cluster, &gvr));
            self._subscriptions
                .push(cx.observe(handle.entity(), |_, _, cx| cx.notify()));
            self.all_apps = Some(handle);
        }
        let namespace = app_namespace(&self.target.cluster, &managed, cx);
        let target = ResourceRef::object(
            self.target.cluster.clone(),
            gvr.clone(),
            Some(namespace.clone()),
            managed.app_name.clone(),
        );
        if self.app.as_ref().map(|(t, _)| t) != Some(&target) {
            let handle = ResourceStores::acquire(
                cx,
                StoreKey::new(self.target.cluster.clone(), gvr, Some(namespace))
                    .fields(format!("metadata.name={}", managed.app_name)),
            );
            self._subscriptions
                .push(cx.observe(handle.entity(), |_, _, cx| cx.notify()));
            self.app = Some((target, handle));
        }
    }
}

impl Render for ManagedSection {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let colors = cx.colors().clone();
        let Some(managed) = object_of(&self.target, &self.store, cx).and_then(|o| managed_by(&o))
        else {
            return div().into_any_element();
        };
        let Some((target, handle)) = self.app.clone() else {
            return div().into_any_element();
        };
        let app = object_of(&target, handle.entity(), cx).and_then(|o| Application::parse(&o));
        let app = match app {
            Some(app) => app,
            // Label tracking without such an Application: Helm's label, not Argo CD's.
            None if managed.via_label => return div().into_any_element(),
            None => {
                return widgets::section("Argo CD", &colors)
                    .child(
                        div()
                            .text_size(u(12.0))
                            .text_color(colors.text_dim)
                            .child(format!(
                                "Tracked by Argo CD app {} (not found).",
                                managed.app_name
                            )),
                    )
                    .into_any_element();
            }
        };
        let policy = app.policy();
        let note = if policy.self_heal() {
            "Self-heal is on: Argo CD reverts changes made here."
        } else if policy.auto_sync() {
            "Changes made here show as out of sync; the next sync reverts them."
        } else {
            "Changes made here show as out of sync until the app is synced."
        };
        let open = target.clone();
        widgets::section("Argo CD", &colors)
            .child(
                h_flex()
                    .gap(u(6.0))
                    .text_size(u(12.0))
                    .child("Managed by Argo CD app")
                    .child(widgets::link(
                        "argo-managed-by",
                        app.name().to_string(),
                        &colors,
                        move |_, window, cx| actions::open_app(open.clone(), None, window, cx),
                    )),
            )
            .child(
                h_flex()
                    .gap(u(12.0))
                    .text_size(u(12.0))
                    .child(widgets::sync_pill(app.sync(), &colors))
                    .child(widgets::health_pill(app.health(), &colors)),
            )
            .child(
                div()
                    .text_size(u(12.0))
                    .text_color(colors.text_dim)
                    .child(note),
            )
            .into_any_element()
    }
}

/// The dock summary of an Application.
struct AppDock {
    target: ResourceRef,
    store: Entity<ResourceStore>,
    _own: Option<StoreHandle>,
    _subscriptions: Vec<Subscription>,
}

impl AppDock {
    fn new(target: ResourceRef, cx: &mut Context<Self>) -> Self {
        let (store, own) = object_store(&target, cx);
        let mut subscriptions = vec![cx.observe(&store, |_, _, cx| cx.notify())];
        if let Some(argo) = ArgoCd::try_global(cx) {
            subscriptions.push(cx.observe(&argo, |_, _, cx| cx.notify()));
        }
        Self {
            target,
            store,
            _own: own,
            _subscriptions: subscriptions,
        }
    }
}

impl Render for AppDock {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let colors = cx.colors().clone();
        let Some(app) =
            object_of(&self.target, &self.store, cx).and_then(|o| Application::parse(&o))
        else {
            return div().into_any_element();
        };
        let writable = !run::read_only(&self.target.cluster, cx);
        let api =
            ArgoCd::try_global(cx).is_some_and(|a| a.read(cx).api(&self.target.cluster).is_some());
        let policy = app.policy();
        let (sync_target, history_target, open_target) = (
            self.target.clone(),
            self.target.clone(),
            self.target.clone(),
        );
        let refresh_target = self.target.clone();
        let buttons = h_flex()
            .gap(u(6.0))
            .flex_wrap()
            .when(writable, |this| {
                this.child(
                    Button::new("argo-dock-sync")
                        .primary()
                        .icon(IconName::RefreshCw)
                        .label("Sync…")
                        .on_click(move |_, window, cx| {
                            dialogs::open_sync(sync_target.clone(), window, cx)
                        }),
                )
                .child(Button::new("argo-dock-refresh").label("Refresh").on_click(
                    move |_, _, cx| {
                        run::run(refresh_target.clone(), Op::Refresh { hard: false }, cx).detach()
                    },
                ))
            })
            .child(
                Button::new("argo-dock-history")
                    .ghost()
                    .icon(IconName::History)
                    .label("History")
                    .on_click(move |_, window, cx| {
                        actions::open_app(
                            history_target.clone(),
                            Some(crate::views::app::Tab::History),
                            window,
                            cx,
                        )
                    }),
            )
            .child(
                Button::new("argo-dock-open")
                    .ghost()
                    .label("Open")
                    .on_click(move |_, window, cx| {
                        actions::open_app(open_target.clone(), None, window, cx)
                    }),
            );
        let status = widgets::section("Argo CD application", &colors)
            .child(
                h_flex()
                    .gap(u(10.0))
                    .flex_wrap()
                    .text_size(u(12.0))
                    .child(widgets::sync_pill(app.sync(), &colors))
                    .child(widgets::health_pill(app.health(), &colors))
                    .when_some(app.activity(), |this, activity| {
                        this.child(widgets::activity_chip(&activity, &colors))
                    }),
            )
            .child(buttons);
        let out_of_sync = app.out_of_sync();
        let mut drift = widgets::section(
            format!(
                "Out of sync · {} of {} resources",
                out_of_sync.len(),
                app.status.resources.len()
            ),
            &colors,
        );
        for resource in out_of_sync.iter().take(8) {
            drift = drift.child(
                h_flex()
                    .gap(u(8.0))
                    .text_size(u(12.0))
                    .child(
                        div()
                            .flex_none()
                            .w(u(80.0))
                            .truncate()
                            .text_color(colors.text_dim)
                            .child(resource.kind.clone()),
                    )
                    .child(
                        div()
                            .flex_1()
                            .min_w_0()
                            .truncate()
                            .font_family(fonts::MONO)
                            .text_size(u(11.5))
                            .child(resource.name.clone()),
                    )
                    .child(widgets::sync_pill(SyncStatus::OutOfSync, &colors)),
            );
        }
        if !out_of_sync.is_empty() {
            let cluster = self.target.cluster.clone();
            let diff_target = self.target.clone();
            drift = drift.child(if api {
                widgets::link(
                    "argo-dock-diff",
                    "Show the diff",
                    &colors,
                    move |_, window, cx| {
                        actions::open_app(
                            diff_target.clone(),
                            Some(crate::views::app::Tab::Diff),
                            window,
                            cx,
                        )
                    },
                )
                .into_any_element()
            } else {
                h_flex()
                    .gap(u(4.0))
                    .text_size(u(11.5))
                    .text_color(colors.text_dim)
                    .child("The diff needs API mode.")
                    .child(widgets::link(
                        "argo-dock-sign-in",
                        "Sign in to Argo CD…",
                        &colors,
                        move |_, window, cx| dialogs::open_sign_in(cluster.clone(), window, cx),
                    ))
                    .into_any_element()
            });
        }
        let source = app.spec.all_sources().into_iter().next();
        let mut facts: Vec<(&'static str, gpui::AnyElement)> = Vec::new();
        if let Some(source) = &source {
            facts.push(("Repository", widgets::kv_text(source.repo_short())));
            facts.push(("Path", widgets::kv_mono(source.what())));
            facts.push(("Target", widgets::kv_mono(source.target().to_string())));
        }
        facts.push((
            "Destination",
            widgets::kv_text(app.spec.destination.label()),
        ));
        facts.push(("Project", widgets::kv_text(app.spec.project.clone())));
        let source_section = widgets::section("Source", &colors).child(widgets::kv(facts, &colors));
        let toggle = |id: &'static str,
                      label: &'static str,
                      on: bool,
                      enabled: bool,
                      change: fn(bool) -> PolicyChange| {
            let target = self.target.clone();
            h_flex()
                .gap(u(8.0))
                .text_size(u(12.0))
                .child(div().flex_1().text_color(colors.text_muted).child(label))
                .child(widgets::toggle(
                    id,
                    on,
                    enabled,
                    &colors,
                    move |_, window, cx| {
                        dialogs::run_confirmed(target.clone(), Op::Policy(change(!on)), window, cx)
                    },
                ))
        };
        let policy_section = widgets::section("Sync policy", &colors)
            .child(toggle(
                "argo-dock-auto",
                "Auto-sync",
                policy.auto_sync(),
                writable,
                PolicyChange::AutoSync,
            ))
            .child(toggle(
                "argo-dock-prune",
                "Prune",
                policy.prune(),
                writable && policy.auto_sync(),
                PolicyChange::Prune,
            ))
            .child(toggle(
                "argo-dock-heal",
                "Self-heal",
                policy.self_heal(),
                writable && policy.auto_sync(),
                PolicyChange::SelfHeal,
            ));
        let last = app.operation_state().map(|state| {
            let by = state.initiator().map(|i| i.label()).unwrap_or_default();
            let phase = state.phase();
            widgets::section("Last operation", &colors).child(
                h_flex()
                    .items_start()
                    .gap(u(8.0))
                    .text_size(u(12.0))
                    .children(phase.map(|p| widgets::result_icon(p, &colors).size(13.0)))
                    .child(
                        v_flex()
                            .child(format!(
                                "{} · {} ago · by {by}",
                                phase.map(|p| p.label()).unwrap_or("?"),
                                widgets::age_of(state.finished().or(state.started()))
                            ))
                            .when_some(state.revision(), |this, r| {
                                this.child(div().text_color(colors.text_dim).child(format!(
                                    "revision {}",
                                    crate::model::short_revision(&r)
                                )))
                            }),
                    ),
            )
        });
        v_flex()
            .child(status)
            .when(!out_of_sync.is_empty(), |this| this.child(drift))
            .child(source_section)
            .child(policy_section)
            .children(last)
            .into_any_element()
    }
}
