//! Flux in other crates' views (board 21):
//! - the details dock of a Flux object: state and message, reconcile/suspend/resume, source and
//!   revision, what it waits for;
//! - "Managed by Flux Kustomization X" (or HelmRelease) in the details of objects Flux applied,
//!   with a link and its state;
//! - the YAML editor's warning that Flux reverts edits of the objects it manages.

use std::sync::Arc;

use gpui::{
    AnyView, App, AppContext as _, Context, Entity, IntoElement, Render, SharedString,
    Subscription, Window, div, prelude::*,
};
use kubyl_core::{ClusterId, DetailsSection, EditNotice, ResourceRef};
use kubyl_flux_core::deps::Graph;
use kubyl_flux_core::kinds::{FluxKind, is_flux_group};
use kubyl_flux_core::model::{FluxObject, ObjectRef, State, short_revision};
use kubyl_flux_core::ops::Action;
use kubyl_flux_core::ownership;
use kubyl_resources::{
    ResourceSelection, ResourceStore, ResourceStores, StoreHandle, StoreKey, object_key,
};
use kubyl_ui::{ActiveColors, Button, IconName, h_flex, u, v_flex};
use serde_json::Value;

use crate::actions;
use crate::state::{self, Flux};
use crate::widgets;

/// What happens to edits of an object `manager` applied.
pub fn revert_note(manager: &ObjectRef, object: Option<&FluxObject>) -> String {
    let every = object
        .and_then(|o| o.interval.clone())
        .map(|i| format!(" (every {i})"))
        .unwrap_or_default();
    match manager.kind {
        Some(FluxKind::HelmRelease) => {
            let drift = object
                .and_then(|o| o.raw.pointer("/spec/driftDetection/mode"))
                .and_then(Value::as_str)
                == Some("enabled");
            if drift {
                format!(
                    "helm-controller corrects drift: it reverts changes made here at its next reconcile{every}."
                )
            } else {
                "helm-controller's next upgrade overwrites changes made here.".to_string()
            }
        }
        _ => {
            format!("kustomize-controller reverts changes made here at its next reconcile{every}.")
        }
    }
}

/// `Kustomization flux-demo/podinfo`.
fn manager_label(manager: &ObjectRef) -> String {
    format!("{} {}", manager.kind_name, manager.key())
}

// ----- Edit notice -----

pub struct FluxNotice;

impl EditNotice for FluxNotice {
    fn notice(&self, target: &ResourceRef, object: &Value, cx: &App) -> Option<SharedString> {
        if !Flux::caps(&target.cluster, cx).any() || is_flux_group(&target.gvr.group) {
            return None;
        }
        let manager = ownership::managed_by(object)?;
        let flux = state::find(
            &target.cluster,
            manager.kind?,
            &manager.namespace,
            &manager.name,
            cx,
        );
        let suspended = flux.as_ref().is_some_and(|f| f.suspended);
        Some(
            if suspended {
                format!(
                    "Managed by Flux {} (suspended): changes made here last until it's resumed.",
                    manager_label(&manager)
                )
            } else {
                format!(
                    "Managed by Flux {}: {}",
                    manager_label(&manager),
                    revert_note(&manager, flux.as_ref())
                )
            }
            .into(),
        )
    }
}

// ----- Details sections -----

pub struct FluxDetails;

impl DetailsSection for FluxDetails {
    fn id(&self) -> &'static str {
        "flux"
    }

    fn order(&self) -> i32 {
        -9
    }

    fn build(&self, target: &ResourceRef, _kind: &str, cx: &mut App) -> Option<AnyView> {
        if !Flux::caps(&target.cluster, cx).any() {
            return None;
        }
        let target = target.clone();
        if let Some(kind) = actions::kind_of(&target) {
            return Some(cx.new(|cx| FluxDock::new(target, kind, cx)).into());
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

/// "Managed by Flux Kustomization X".
struct ManagedSection {
    target: ResourceRef,
    store: Entity<ResourceStore>,
    _own: Option<StoreHandle>,
    /// The manager's watch, once the object names it.
    manager: Option<(ObjectRef, StoreHandle)>,
    /// Observes the manager's watch (replaced with it).
    _manager_observer: Option<Subscription>,
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
            manager: None,
            _manager_observer: None,
            _subscriptions: subscriptions,
        };
        this.sync(cx);
        this
    }

    fn sync(&mut self, cx: &mut Context<Self>) {
        let object = object_of(&self.target, &self.store, cx);
        let Some(manager) = object.as_deref().and_then(ownership::managed_by) else {
            // Flux's labels went away (or the object did): nothing manages it now. While the
            // object isn't loaded yet, keep what's shown.
            if (object.is_some() || self.store.read(cx).status().is_ready())
                && self.manager.take().is_some()
            {
                self._manager_observer = None;
                cx.notify();
            }
            return;
        };
        if self.manager.as_ref().map(|(m, _)| m) == Some(&manager) {
            return;
        }
        let Some(gvr) = manager
            .kind
            .and_then(|k| state::gvr(&self.target.cluster, k, cx))
        else {
            return;
        };
        let handle = ResourceStores::acquire(
            cx,
            StoreKey::new(
                self.target.cluster.clone(),
                gvr,
                Some(manager.namespace.clone()),
            )
            .fields(format!("metadata.name={}", manager.name)),
        );
        self._manager_observer = Some(cx.observe(handle.entity(), |_, _, cx| cx.notify()));
        self.manager = Some((manager, handle));
    }
}

impl Render for ManagedSection {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let colors = cx.colors().clone();
        let Some((manager, handle)) = self.manager.clone() else {
            return div().into_any_element();
        };
        let Some(kind) = manager.kind else {
            return div().into_any_element();
        };
        let object = handle
            .read(cx)
            .get(&object_key(Some(&manager.namespace), &manager.name))
            .and_then(|o| FluxObject::parse_as(kind, o));
        let target = state::gvr(&self.target.cluster, kind, cx).map(|gvr| {
            ResourceRef::object(
                self.target.cluster.clone(),
                gvr,
                Some(manager.namespace.clone()),
                manager.name.clone(),
            )
        });
        let note = match &object {
            Some(o) if o.suspended => {
                "It's suspended: changes made here last until it's resumed.".to_string()
            }
            _ => revert_note(&manager, object.as_ref()),
        };
        widgets::section("Flux", &colors)
            .child(
                h_flex()
                    .gap(u(6.0))
                    .text_size(u(12.0))
                    .child(format!("Managed by Flux {}", manager.kind_name))
                    .child(match target {
                        Some(target) => widgets::link(
                            "flux-managed-by",
                            manager.key(),
                            &colors,
                            move |_, window, cx| actions::open_object(target.clone(), window, cx),
                        )
                        .into_any_element(),
                        None => div().child(manager.key()).into_any_element(),
                    }),
            )
            .when_some(object.as_ref(), |this, object| {
                this.child(
                    h_flex()
                        .gap(u(10.0))
                        .text_size(u(12.0))
                        .child(widgets::state_pill(object.state(), &colors))
                        .when_some(object.revision(), |this, r| {
                            this.child(widgets::mono(short_revision(&r)))
                        }),
                )
            })
            .when(
                object.is_none() && handle.read(cx).status().is_ready(),
                |this| {
                    this.child(
                        div().text_size(u(12.0)).text_color(colors.yellow).child(
                            "The Flux object doesn't exist (anymore): nothing manages this.",
                        ),
                    )
                },
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

/// The dock summary of a Flux object.
struct FluxDock {
    target: ResourceRef,
    kind: FluxKind,
    store: Entity<ResourceStore>,
    _own: Option<StoreHandle>,
    /// The kind in all namespaces (the Flux entity keeps Kustomizations and HelmReleases
    /// watched), for "Waiting for"; observed so the line follows the dependencies.
    same_kind: Option<Entity<ResourceStore>>,
    parsed: state::ParsedObjects,
    _subscriptions: Vec<Subscription>,
}

impl FluxDock {
    fn new(target: ResourceRef, kind: FluxKind, cx: &mut Context<Self>) -> Self {
        let (store, own) = object_store(&target, cx);
        let mut subscriptions = vec![cx.observe(&store, |_, _, cx| cx.notify())];
        if let Some(flux) = Flux::try_global(cx) {
            subscriptions.push(cx.observe(&flux, |_, _, cx| cx.notify()));
        }
        let same_kind = state::gvr(&target.cluster, kind, cx)
            .and_then(|gvr| ResourceStores::peek(cx, &state::all_key(&target.cluster, &gvr)));
        if let Some(same) = &same_kind {
            subscriptions.push(cx.observe(same, |_, _, cx| cx.notify()));
        }
        Self {
            target,
            kind,
            store,
            _own: own,
            same_kind,
            parsed: state::ParsedObjects::default(),
            _subscriptions: subscriptions,
        }
    }

    fn cluster(&self) -> &ClusterId {
        &self.target.cluster
    }
}

impl Render for FluxDock {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let colors = cx.colors().clone();
        let Some(object) = object_of(&self.target, &self.store, cx)
            .and_then(|o| FluxObject::parse_as(self.kind, &o))
        else {
            return div().into_any_element();
        };
        let writable = !state::read_only(self.cluster(), cx);
        let state = object.state();
        let target = self.target.clone();
        let mut buttons = h_flex().gap(u(6.0)).flex_wrap();
        if writable && Action::Reconcile.applies_to(&object) {
            let t = target.clone();
            buttons = buttons.child(
                Button::new("flux-dock-reconcile")
                    .primary()
                    .icon(IconName::RefreshCw)
                    .label("Reconcile")
                    .on_click(move |_, window, cx| {
                        actions::request(vec![t.clone()], Action::Reconcile, window, cx)
                    }),
            );
        }
        if writable && object.suspendable() {
            let t = target.clone();
            let (action, icon) = if object.suspended {
                (Action::Resume, IconName::Play)
            } else {
                (Action::Suspend, IconName::Pause)
            };
            buttons = buttons.child(
                Button::new("flux-dock-suspend")
                    .icon(icon)
                    .label(action.label())
                    .on_click(move |_, window, cx| {
                        actions::request(vec![t.clone()], action, window, cx)
                    }),
            );
        }
        let open = target.clone();
        buttons = buttons.child(
            Button::new("flux-dock-open")
                .ghost()
                .label("Open")
                .on_click(move |_, window, cx| actions::open_object(open.clone(), window, cx)),
        );
        let status = widgets::section(format!("Flux {}", self.kind), &colors)
            .child(
                h_flex()
                    .gap(u(10.0))
                    .text_size(u(12.0))
                    .child(widgets::state_pill(state, &colors))
                    .when_some(object.revision(), |this, r| {
                        this.child(widgets::mono(short_revision(&r)))
                    }),
            )
            .child(
                div()
                    .text_size(u(12.0))
                    .text_color(if state.is_problem() {
                        colors.red
                    } else {
                        colors.text_muted
                    })
                    .child(object.message()),
            )
            .child(buttons);
        let mut facts: Vec<(&'static str, gpui::AnyElement)> = Vec::new();
        if let Some(source) = &object.source {
            facts.push(("Source", widgets::kv_mono(source.label(&object.namespace))));
        }
        if let Some(interval) = &object.interval {
            facts.push(("Interval", widgets::kv_mono(interval.clone())));
        }
        if let Some(last) = object.last_reconcile() {
            facts.push((
                "Last change",
                widgets::kv_text(format!("{} ago", widgets::age(Some(&last)))),
            ));
        }
        if let Some(entries) = kubyl_flux_core::inventory::entries(&object.raw) {
            facts.push((
                "Applied",
                widgets::kv_text(format!("{} objects", entries.len())),
            ));
        }
        // Only objects that aren't Ready (or suspended) wait, as in the overview.
        let waiting = match &self.same_kind {
            Some(same) if !object.depends_on.is_empty() => {
                let same = self.parsed.of(self.kind, same, cx);
                if same.is_empty() {
                    None
                } else {
                    Graph::new(same.iter()).waiting(&object)
                }
            }
            _ => None,
        };
        v_flex()
            .child(status)
            .when_some(waiting, |this, dep| {
                this.child(
                    widgets::section("Dependencies", &colors).child(
                        div()
                            .text_size(u(12.0))
                            .text_color(colors.yellow)
                            .child(format!(
                                "Waiting for {} ({})",
                                dep.key,
                                dep.state.map_or("not found", State::label)
                            )),
                    ),
                )
            })
            .when(!facts.is_empty(), |this| {
                this.child(widgets::section("Details", &colors).child(widgets::kv(facts, &colors)))
            })
            .into_any_element()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use kubyl_flux_core::fixtures;

    #[test]
    fn revert_notes() {
        let ks = FluxObject::parse(&Arc::new(fixtures::kustomization_ready())).unwrap();
        let manager = ownership::managed_by(&fixtures::managed_deployment()).unwrap();
        assert_eq!(
            revert_note(&manager, Some(&ks)),
            "kustomize-controller reverts changes made here at its next reconcile (every 10m)."
        );
        let hr = ownership::managed_by(&fixtures::helm_deployment()).unwrap();
        assert!(revert_note(&hr, None).contains("next upgrade overwrites"));
        let mut drift = fixtures::helm_release_v2();
        drift["spec"]["driftDetection"] = serde_json::json!({"mode": "enabled"});
        let drift = FluxObject::parse(&Arc::new(drift)).unwrap();
        assert!(revert_note(&hr, Some(&drift)).contains("corrects drift"));
    }
}
