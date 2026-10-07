//! The Flux tabs (board 21): the overview, a list per category (Kustomizations, HelmReleases,
//! Sources, Image Automation, Notifications) and an object's tab.

pub mod list;
pub mod object;
pub mod overview;

#[cfg(test)]
mod tests;

use gpui::{App, AppContext as _, KeyBinding, SharedString, actions};
use kubyl_core::{
    ActionRegistry, ClusterCaps, ClusterId, Gvr, ResourceRef, ViewKind, ViewRegistry,
};
use kubyl_flux_core::kinds::{Category, FluxKind};
use kubyl_flux_core::model::FluxObject;
use kubyl_flux_core::ops::Action;

actions!(
    flux_views,
    [
        /// Focuses the filter field of a Flux view.
        FocusFilter,
    ]
);

pub(crate) fn init(cx: &mut App) {
    cx.bind_keys([KeyBinding::new(
        "/",
        FocusFilter,
        Some(crate::actions::LIST_CONTEXT),
    )]);
    ViewRegistry::register(
        cx,
        ViewKind::Custom(overview::VIEW_KIND.into()),
        |request, window, cx| {
            let target = request.target.clone()?;
            Some(Box::new(cx.new(|cx| {
                overview::OverviewView::new(target.cluster, window, cx)
            })))
        },
    );
    for category in Category::ALL {
        ViewRegistry::register(
            cx,
            ViewKind::Custom(category.view_id().into()),
            move |request, window, cx| {
                let target = request.target.clone()?;
                Some(Box::new(
                    cx.new(|cx| list::ListView::new(category, target, window, cx)),
                ))
            },
        );
    }
    ViewRegistry::register(
        cx,
        ViewKind::Custom(object::VIEW_KIND.into()),
        |request, window, cx| {
            let target = request.target.clone().filter(ResourceRef::is_object)?;
            Some(Box::new(
                cx.new(|cx| object::ObjectView::new(target, window, cx)),
            ))
        },
    );
    // The explorer, the palette (`:ks`, `:hr`) and `open_selected` open Flux kinds in these
    // views; their objects open in an object's tab.
    for kind in FluxKind::ALL {
        ViewRegistry::register_list_view(
            cx,
            kind.group(),
            kind.plural(),
            ViewKind::Custom(kind.category().view_id().into()),
        );
        ViewRegistry::register_object_view(
            cx,
            kind.group(),
            kind.plural(),
            ViewKind::Custom(object::VIEW_KIND.into()),
        );
    }
}

/// The ref a category view is opened for (no kind).
pub fn category_ref(cluster: &ClusterId, namespace: Option<String>) -> ResourceRef {
    ResourceRef::list(cluster.clone(), Gvr::new("", "", ""), namespace)
}

/// Key hints of `context` whose action applies to `target` on a cluster with `caps`: writing
/// actions are hidden on read-only clusters, and with the loaded `object` those that don't
/// apply to it (Reconcile on a static OCI HelmRepository, Resume on a running object).
pub fn hints(
    context: &str,
    target: Option<&ResourceRef>,
    object: Option<&FluxObject>,
    caps: &ClusterCaps,
    cx: &App,
) -> Vec<(SharedString, SharedString)> {
    ActionRegistry::global(cx)
        .all()
        .iter()
        .filter(|spec| spec.context.as_deref() == Some(context))
        .filter(|spec| target.is_none_or(|t| spec.is_available(t, caps)))
        .filter(|spec| object.is_none_or(|o| action_of(&spec.name).is_none_or(|a| a.applies_to(o))))
        .filter(|spec| target.is_some() || !caps.read_only || !writes(&spec.name))
        .filter_map(|spec| Some((spec.keystrokes.clone()?, spec.hint.clone()?)))
        .collect()
}

/// The Flux action behind an action's palette name.
fn action_of(name: &str) -> Option<Action> {
    let name = name.strip_prefix("Flux: ")?.trim_end_matches('…');
    [
        Action::Reconcile,
        Action::ReconcileWithSource,
        Action::Force,
        Action::Reset,
        Action::Suspend,
        Action::Resume,
        Action::Delete,
    ]
    .into_iter()
    .find(|a| match a {
        Action::Reconcile => name == "Reconcile",
        Action::ReconcileWithSource => name == "Reconcile With Source",
        Action::Force => name == "Force Upgrade",
        Action::Reset => name == "Reset Failures",
        Action::Suspend => name == "Suspend",
        Action::Resume => name == "Resume",
        Action::Delete => name == "Delete",
    })
}

/// Whether an action's name is one that writes (for hints without a selection).
fn writes(name: &str) -> bool {
    ["Reconcile", "Suspend", "Resume", "Delete", "Force", "Reset"]
        .iter()
        .any(|w| name.contains(w))
}
