//! Helm in other views: "Managed by Helm release X" in the details of objects a release manages
//! (`app.kubernetes.io/managed-by: Helm` and `meta.helm.sh/release-name`), with a link to the
//! release, and the YAML editor's warning that the next upgrade reverts edits.

use gpui::{
    AnyView, App, AppContext as _, Context, Entity, IntoElement, Render, SharedString,
    Subscription, Window, div, prelude::*,
};
use kubyl_core::{DetailsSection, EditNotice, ResourceRef};
use kubyl_helm_core::release::managed_by;
use kubyl_resources::{
    ResourceSelection, ResourceStore, ResourceStores, StoreHandle, StoreKey, object_key,
};
use kubyl_ui::{ActiveColors, IconName, fonts, h_flex, u, v_flex};
use serde_json::Value;

use crate::service::{Helm, HelmLease};
use crate::widgets;

/// The YAML editor's warning.
pub struct HelmNotice;

impl EditNotice for HelmNotice {
    fn notice(&self, _: &ResourceRef, object: &Value, _: &App) -> Option<SharedString> {
        let (namespace, name) = managed_by(object)?;
        Some(
            format!(
                "Managed by Helm release {namespace}/{name}: the next helm upgrade reverts changes made here. Change the release's values instead."
            )
            .into(),
        )
    }
}

/// The details section.
pub struct HelmDetails;

impl DetailsSection for HelmDetails {
    fn id(&self) -> &'static str {
        "helm"
    }

    fn order(&self) -> i32 {
        -9
    }

    fn build(&self, target: &ResourceRef, _: &str, cx: &mut App) -> Option<AnyView> {
        let target = target.clone();
        // Helm's own storage objects aren't managed by a release.
        if target.gvr.group.is_empty()
            && matches!(target.gvr.resource.as_str(), "secrets" | "configmaps")
            && target
                .name
                .as_deref()
                .is_some_and(|n| n.starts_with("sh.helm.release."))
        {
            return None;
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

struct ManagedSection {
    target: ResourceRef,
    store: Entity<ResourceStore>,
    _own: Option<StoreHandle>,
    /// The cluster's release watches, once the object names a release.
    lease: Option<HelmLease>,
    _subscriptions: Vec<Subscription>,
}

impl ManagedSection {
    fn new(target: ResourceRef, cx: &mut Context<Self>) -> Self {
        let (store, own) = object_store(&target, cx);
        let mut subscriptions = vec![cx.observe(&store, |this, _, cx| {
            this.sync(cx);
            cx.notify();
        })];
        if let Some(helm) = Helm::global(cx) {
            subscriptions.push(cx.observe(&helm, |_, _, cx| cx.notify()));
        }
        let mut this = Self {
            target,
            store,
            _own: own,
            lease: None,
            _subscriptions: subscriptions,
        };
        this.sync(cx);
        this
    }

    fn object(&self, cx: &App) -> Option<std::sync::Arc<Value>> {
        let key = object_key(
            self.target.namespace.as_deref(),
            self.target.name.as_deref()?,
        );
        self.store.read(cx).get(&key).cloned()
    }

    fn sync(&mut self, cx: &mut Context<Self>) {
        if self.lease.is_none() && self.object(cx).and_then(|o| managed_by(&o)).is_some() {
            self.lease = Helm::watch(&self.target.cluster, cx);
        }
    }
}

impl Render for ManagedSection {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let colors = cx.colors().clone();
        let Some((namespace, name)) = self.object(cx).and_then(|o| managed_by(&o)) else {
            return div().into_any_element();
        };
        let row = crate::releases::snapshot(&self.target.cluster, cx).and_then(|s| {
            s.releases
                .iter()
                .find(|r| r.namespace == namespace && r.name == name)
                .cloned()
        });
        let cluster = self.target.cluster.clone();
        let label = format!("{namespace}/{name}");
        let link = match row {
            Some(row) => {
                let status = row.latest().status.clone();
                h_flex()
                    .gap(u(6.0))
                    .child(
                        widgets::link("helm-managed-link", label, &colors, move |_, window, cx| {
                            crate::release::open(&cluster, &row, None, window, cx)
                        })
                        .font_family(fonts::MONO)
                        .text_size(u(12.0)),
                    )
                    .child(widgets::pill(
                        status.clone(),
                        crate::releases::status_tone(&status),
                        &colors,
                    ))
                    .into_any_element()
            }
            None => div()
                .font_family(fonts::MONO)
                .text_size(u(12.0))
                .child(label)
                .into_any_element(),
        };
        widgets::section("Helm", &colors)
            .child(
                v_flex()
                    .gap(u(6.0))
                    .child(
                        h_flex()
                            .gap(u(6.0))
                            .text_size(u(12.0))
                            .child("Managed by Helm release")
                            .child(link),
                    )
                    .child(widgets::note(
                        IconName::Info,
                        colors.accent,
                        "The next helm upgrade reverts changes made here: change the release's values instead.",
                        &colors,
                    )),
            )
            .into_any_element()
    }
}
