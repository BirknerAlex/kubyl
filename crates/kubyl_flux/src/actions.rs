//! Flux actions: in the palette (`> Flux: Reconcile`, `Suspend`, `Resume`…), bound to keys in
//! the Flux lists (`FluxList`), an object's tab (`FluxObject`) and the explorer's generic
//! tables of Flux kinds. They act on the selection (several objects at once for reconcile,
//! suspend and resume); an object's tab handles them for its own object.
//!
//! Every action is what the `flux` CLI does, as a patch with the user's own access
//! ([`kubyl_flux_core::ops`]): checked with `can_i` first, hidden on read-only clusters,
//! confirmed on PROD (deleting needs the typed name there), suspend, force and delete confirmed
//! everywhere. The result shows in the objects' state (Reconciling → Ready or Failed).

use gpui::{App, Window, actions};
use kubyl_core::actions::OpenView;
use kubyl_core::{
    ActionRegistry, ActionSpec, ClusterCaps, ClusterId, Gvr, Notification, NotificationCenter,
    ResourceRef, ViewKind, ViewRequest, spawn_kube,
};
use kubyl_explorer::dialogs::{self, ConfirmSpec};
use std::collections::BTreeSet;

use kubyl_flux_core::inventory::{self, Entry};
use kubyl_flux_core::kinds::{Category, FluxKind, is_flux_group};
use kubyl_flux_core::model::FluxObject;
use kubyl_flux_core::ops::{self, Action, DeleteEffect, OpError, Target};
use kubyl_kube::ConnectionManager;
use kubyl_kube::access::AccessQuery;
use kubyl_resources::ResourceSelection;

use crate::state::{self, Flux};
use crate::views;

actions!(
    flux,
    [
        /// Asks the controller to reconcile the selected objects now.
        Reconcile,
        /// Reconciles the selected objects' sources first, then the objects.
        ReconcileWithSource,
        /// HelmRelease: upgrades even without changes.
        ForceUpgrade,
        /// HelmRelease: resets the install and upgrade failure counters.
        ResetFailures,
        /// Suspends the selected objects (spec.suspend).
        Suspend,
        /// Resumes the selected objects.
        Resume,
        /// Deletes the selected object (what prune removes is listed first).
        Delete,
        /// Opens the selected object in the YAML editor.
        EditYaml,
        /// Opens the selected object's tab.
        OpenObject,
        /// Opens the controller's logs for the selected object.
        ControllerLogs,
        /// Opens the Flux overview of the selection's cluster.
        OpenOverview,
        /// Opens the Kustomizations of the selection's cluster.
        OpenKustomizations,
        /// Opens the HelmReleases of the selection's cluster.
        OpenHelmReleases,
        /// Opens the sources of the selection's cluster.
        OpenSources,
        /// Looks for Flux's controllers again.
        DetectAgain,
    ]
);

/// Key context of the Flux lists.
pub const LIST_CONTEXT: &str = "FluxList";
/// Key context of an object's tab.
pub const OBJECT_CONTEXT: &str = "FluxObject";

/// The target is a Flux object on a cluster that serves Flux.
pub fn is_flux(target: &ResourceRef, caps: &ClusterCaps) -> bool {
    caps.flux.any() && is_flux_group(&target.gvr.group) && target.is_object()
}

fn writable(target: &ResourceRef, caps: &ClusterCaps) -> bool {
    is_flux(target, caps) && !caps.read_only
}

/// By kind and API version (static objects of the served version take no requests; an OCI
/// HelmRepository is caught per object by [`Action::applies_to`]).
fn reconcilable(target: &ResourceRef, caps: &ClusterCaps) -> bool {
    writable(target, caps)
        && kind_of(target).is_some_and(|k| k.reconcilable_at(&target.gvr.version))
}

fn suspendable(target: &ResourceRef, caps: &ClusterCaps) -> bool {
    writable(target, caps) && kind_of(target).is_some_and(|k| k.suspendable_at(&target.gvr.version))
}

fn helm_release(target: &ResourceRef, caps: &ClusterCaps) -> bool {
    writable(target, caps) && kind_of(target) == Some(FluxKind::HelmRelease)
}

/// The Flux kind of a ref.
pub fn kind_of(target: &ResourceRef) -> Option<FluxKind> {
    FluxKind::from_plural(&target.gvr.group, &target.gvr.resource)
}

/// The explorer's generic table of a Flux kind.
fn list_context(kind: FluxKind) -> String {
    format!("ResourceList && kind == {}", kind.kind())
}

pub(crate) fn init(cx: &mut App) {
    type Pred = fn(&ResourceRef, &ClusterCaps) -> bool;
    // (spec, keys, availability, also in the explorer's generic tables)
    let specs: Vec<(ActionSpec, Option<&str>, Pred, bool)> = vec![
        (
            ActionSpec::new("Flux: Reconcile", Reconcile).hint("Reconcile"),
            Some("r"),
            reconcilable,
            true,
        ),
        (
            ActionSpec::new("Flux: Reconcile With Source", ReconcileWithSource).hint("With source"),
            Some("shift-r"),
            reconcilable,
            true,
        ),
        (
            ActionSpec::new("Flux: Suspend…", Suspend).hint("Suspend"),
            Some("s"),
            suspendable,
            true,
        ),
        (
            ActionSpec::new("Flux: Resume", Resume).hint("Resume"),
            Some("u"),
            suspendable,
            true,
        ),
        (
            ActionSpec::new("Flux: Force Upgrade…", ForceUpgrade),
            None,
            helm_release,
            true,
        ),
        (
            ActionSpec::new("Flux: Reset Failures", ResetFailures),
            None,
            helm_release,
            true,
        ),
        (
            ActionSpec::new("Flux: Controller Logs", ControllerLogs).hint("Logs"),
            Some("l"),
            is_flux,
            false,
        ),
        // The generic tables have their own Edit YAML (`e`) and Delete (`ctrl-d`).
        (
            ActionSpec::new("Flux: Edit YAML", EditYaml).hint("Edit YAML"),
            Some("e"),
            is_flux,
            false,
        ),
        (
            ActionSpec::new("Flux: Delete…", Delete).hint("Delete…"),
            Some("ctrl-d"),
            writable,
            false,
        ),
        (
            ActionSpec::new("Flux: Open", OpenObject),
            None,
            is_flux,
            true,
        ),
    ];
    let mut contexts: Vec<String> = vec![LIST_CONTEXT.into(), OBJECT_CONTEXT.into()];
    contexts.extend(FluxKind::ALL.into_iter().map(list_context));
    for (spec, keys, available, in_generic) in specs {
        for context in &contexts {
            let generic = context.starts_with("ResourceList");
            if generic && !in_generic {
                continue;
            }
            let mut spec = spec.clone().available_when(available);
            match keys {
                Some(keys) => spec = spec.bind(keys, Some(context)),
                None => spec.context = Some(context.clone().into()),
            }
            ActionRegistry::register(cx, spec);
        }
    }
    for (spec, needs) in [
        (
            ActionSpec::new("Flux: Open Overview", OpenOverview),
            (|_, caps| caps.flux.any()) as Pred,
        ),
        (
            ActionSpec::new("Flux: Open Kustomizations", OpenKustomizations),
            |_, caps| caps.flux.kustomizations,
        ),
        (
            ActionSpec::new("Flux: Open HelmReleases", OpenHelmReleases),
            |_, caps| caps.flux.helm_releases,
        ),
        (
            ActionSpec::new("Flux: Open Sources", OpenSources),
            |_, caps| caps.flux.sources,
        ),
        (
            ActionSpec::new("Flux: Look for Controllers Again", DetectAgain),
            |_, caps| caps.flux.any(),
        ),
    ] {
        ActionRegistry::register(cx, spec.available_when(needs));
    }

    cx.on_action(|_: &Reconcile, cx| on_selection(Action::Reconcile, cx));
    cx.on_action(|_: &ReconcileWithSource, cx| on_selection(Action::ReconcileWithSource, cx));
    cx.on_action(|_: &ForceUpgrade, cx| on_selection(Action::Force, cx));
    cx.on_action(|_: &ResetFailures, cx| on_selection(Action::Reset, cx));
    cx.on_action(|_: &Suspend, cx| on_selection(Action::Suspend, cx));
    cx.on_action(|_: &Resume, cx| on_selection(Action::Resume, cx));
    cx.on_action(|_: &Delete, cx| on_selection(Action::Delete, cx));
    cx.on_action(|_: &EditYaml, cx| {
        if let Some(target) = selected(cx).into_iter().next() {
            with_window(cx, move |window, cx| edit_yaml(target, window, cx));
        }
    });
    cx.on_action(|_: &OpenObject, cx| {
        if let Some(target) = selected(cx).into_iter().next() {
            with_window(cx, move |window, cx| open_object(target, window, cx));
        }
    });
    cx.on_action(|_: &ControllerLogs, cx| {
        if let Some(target) = selected(cx).into_iter().next() {
            with_window(cx, move |window, cx| controller_logs(&target, window, cx));
        }
    });
    fn open_in_window(category: Option<Category>, cx: &mut App) {
        let Some(cluster) = selection_cluster(cx) else {
            return;
        };
        with_window(cx, move |window, cx| match category {
            Some(category) => open_category(&cluster, category, None, window, cx),
            None => views::overview::open(&cluster, window, cx),
        });
    }
    cx.on_action(|_: &OpenOverview, cx| open_in_window(None, cx));
    cx.on_action(|_: &OpenKustomizations, cx| open_in_window(Some(Category::Kustomizations), cx));
    cx.on_action(|_: &OpenHelmReleases, cx| open_in_window(Some(Category::HelmReleases), cx));
    cx.on_action(|_: &OpenSources, cx| open_in_window(Some(Category::Sources), cx));
    cx.on_action(|_: &DetectAgain, cx| {
        if let Some(cluster) = selection_cluster(cx)
            && let Some(flux) = Flux::try_global(cx)
        {
            flux.update(cx, |flux, cx| flux.redetect(&cluster, cx));
        }
    });
}

/// The selected Flux objects (the primary first).
pub fn selected(cx: &App) -> Vec<ResourceRef> {
    let selection = ResourceSelection::global(cx);
    selection
        .targets()
        .filter(|t| is_flux_group(&t.gvr.group) && t.is_object())
        .cloned()
        .collect()
}

fn selection_cluster(cx: &App) -> Option<ClusterId> {
    ResourceSelection::global(cx)
        .primary()
        .map(|s| s.target.cluster.clone())
        .or_else(|| {
            kubyl_core::ActiveContext::global(cx)
                .cluster
                .as_ref()
                .map(|c| c.id.clone())
        })
}

/// Runs `f` in the focused window (deferred: global handlers run inside the window update).
pub fn with_window(cx: &mut App, f: impl FnOnce(&mut Window, &mut App) + 'static) {
    cx.defer(move |cx| {
        let window = cx.active_window().or_else(|| cx.windows().first().copied());
        if let Some(window) = window
            && let Err(err) = window.update(cx, |_, window, cx| f(window, cx))
        {
            tracing::warn!("no window for the action: {err:#}");
        }
    });
}

fn on_selection(action: Action, cx: &mut App) {
    let targets = selected(cx);
    if targets.is_empty() {
        return;
    }
    with_window(cx, move |window, cx| request(targets, action, window, cx));
}

/// Asks for confirmation where needed, then runs `action` on `targets`.
pub fn request(mut targets: Vec<ResourceRef>, action: Action, window: &mut Window, cx: &mut App) {
    let Some(cluster) = targets.first().map(|t| t.cluster.clone()) else {
        return;
    };
    // A selection in Favorites can span clusters: act on the primary's cluster only (its
    // client, read-only and PROD flags), never on another cluster's objects.
    let before = targets.len();
    targets.retain(|t| t.cluster == cluster);
    if targets.len() < before {
        NotificationCenter::push(
            cx,
            Notification::error(format!(
                "{}: {} selected objects on other clusters were skipped; Flux actions run on one cluster at a time ({cluster}).",
                action.label(),
                before - targets.len()
            )),
        );
    }
    let caps = state::cluster_caps(&cluster, cx);
    if caps.read_only {
        NotificationCenter::push(
            cx,
            Notification::error("This cluster is read-only in Kubyl."),
        );
        return;
    }
    let objects: Vec<(ResourceRef, FluxObject)> = targets
        .into_iter()
        .filter_map(|t| {
            let kind = kind_of(&t)?;
            let object = state::find(
                &t.cluster,
                kind,
                t.namespace.as_deref().unwrap_or_default(),
                t.name.as_deref()?,
                cx,
            )?;
            Some((t, object))
        })
        .filter(|(_, o)| action.applies_to(o))
        .collect();
    if objects.is_empty() {
        NotificationCenter::push(
            cx,
            Notification::error(format!(
                "{} doesn't apply to the selection.",
                action.label()
            )),
        );
        return;
    }
    // Delete acts on one object at a time (its summary is about that object).
    if action == Action::Delete && objects.len() > 1 {
        NotificationCenter::push(
            cx,
            Notification::error(format!(
                "Delete works on one Flux object at a time ({} selected): select only the one to delete.",
                objects.len()
            )),
        );
        return;
    }
    if !action.needs_confirmation() && !caps.production {
        run(objects, action, cx);
        return;
    }
    let kept = if action == Action::Delete {
        kept_entries(&cluster, &objects[0].1, cx)
    } else {
        BTreeSet::new()
    };
    let spec = confirmation(&objects, action, caps.production, &kept);
    dialogs::confirm(
        spec,
        move |_, _, cx| run(objects.clone(), action, cx),
        window,
        cx,
    );
}

/// Inventory entries of `object` that Kubyl's running watches show marked to stay when it's
/// deleted ([`inventory::kept_on_delete`]); entries no watch has loaded aren't known.
fn kept_entries(cluster: &ClusterId, object: &FluxObject, cx: &App) -> BTreeSet<Entry> {
    let DeleteEffect::Prunes { entries, .. } = ops::delete_effect(object) else {
        return BTreeSet::new();
    };
    entries
        .into_iter()
        .filter(|entry| {
            state::served_gvr(cluster, &entry.group, &entry.kind, cx)
                .and_then(|gvr| {
                    state::peek_object(cluster, &gvr, entry.namespace(), &entry.name, cx)
                })
                .is_some_and(|o| inventory::kept_on_delete(&o))
        })
        .collect()
}

/// The dialog for `action` on `objects` (board 21's confirmations). `kept`: inventory entries
/// known to stay when deleting.
pub fn confirmation(
    objects: &[(ResourceRef, FluxObject)],
    action: Action,
    production: bool,
    kept: &BTreeSet<Entry>,
) -> ConfirmSpec {
    let names: Vec<String> = objects
        .iter()
        .map(|(_, o)| format!("{} {}", o.kind, o.key()))
        .collect();
    let what = if names.len() == 1 {
        names[0].clone()
    } else {
        format!("{} objects", names.len())
    };
    let mut spec = ConfirmSpec::new(
        format!("{} {what}?", action.label()),
        match action {
            Action::Delete => "Delete".to_string(),
            other => other.label().to_string(),
        },
    );
    spec.lines = names.iter().take(12).map(|n| n.clone().into()).collect();
    if names.len() > 12 {
        spec.lines
            .push(format!("… and {} more", names.len() - 12).into());
    }
    let one = names.len() == 1;
    let (them, they) = if one {
        ("it", "it's")
    } else {
        ("them", "they're")
    };
    spec.note = Some(
        match action {
            Action::Suspend => format!("Flux stops reconciling {them} until {they} resumed: changes in Git aren't applied and drift isn't corrected."),
            Action::Resume => format!("Flux reconciles {them} again right away."),
            Action::Reconcile | Action::ReconcileWithSource => {
                format!("Sets reconcile.fluxcd.io/requestedAt; the controller reconciles {them} now.")
            }
            Action::Force => "helm-controller upgrades the release even without changes (reconcile.fluxcd.io/forceAt).".to_string(),
            Action::Reset => "Resets the install and upgrade failure counters (reconcile.fluxcd.io/resetAt).".to_string(),
            Action::Delete => ops::delete_note(&objects[0].1, kept.len()),
        }
        .into(),
    );
    spec.danger = matches!(action, Action::Delete | Action::Suspend | Action::Force);
    if action == Action::Delete {
        let removed = match ops::delete_effect(&objects[0].1) {
            DeleteEffect::Prunes { entries, .. } => Some(("prunes", entries)),
            DeleteEffect::Uninstalls { entries, .. } => Some(("uninstalls", entries)),
            _ => None,
        };
        if let Some((verb, entries)) = removed {
            let entries: Vec<Entry> = entries.into_iter().filter(|e| !kept.contains(e)).collect();
            spec.lines = entries
                .iter()
                .take(15)
                .map(|e| format!("{verb} {}", e.label()).into())
                .collect();
            if entries.len() > 15 {
                spec.lines
                    .push(format!("… and {} more", entries.len() - 15).into());
            }
            spec.lines.extend(
                kept.iter()
                    .take(5)
                    .map(|e| format!("keeps {} (marked to stay)", e.label()).into()),
            );
        }
        if production {
            spec.typed = Some(objects[0].1.name.clone());
        }
    }
    spec
}

/// Runs `action` on each object (with `can_i` first) and reports in a toast. The objects'
/// state shows the result as the controller acts.
pub fn run(objects: Vec<(ResourceRef, FluxObject)>, action: Action, cx: &mut App) {
    let Some(cluster) = objects.first().map(|(t, _)| t.cluster.clone()) else {
        return;
    };
    let Some(manager) = ConnectionManager::try_global(cx) else {
        return;
    };
    let Some(client) = manager.read(cx).client(&cluster) else {
        NotificationCenter::push(cx, Notification::error("The cluster isn't connected."));
        return;
    };
    let verb = if action == Action::Delete {
        "delete"
    } else {
        "patch"
    };
    struct Job {
        label: String,
        action: Action,
        checks: Vec<(String, gpui::Task<Option<bool>>)>,
        target: Target,
        source: Option<Target>,
    }
    let mut jobs = Vec::new();
    let mut skipped = Vec::new();
    for (_, object) in objects {
        let Some((gvr, resource)) = state::resource(&cluster, object.kind, cx) else {
            skipped.push(format!(
                "{} {}: the cluster doesn't serve {} (anymore)",
                object.kind.short(),
                object.key(),
                object.kind.label()
            ));
            continue;
        };
        let mut job_action = action;
        let mut source_ref = (action == Action::ReconcileWithSource)
            .then(|| ops::source_to_reconcile(&object))
            .flatten();
        // Like `flux reconcile --with-source`: a suspended source stops it (its controller
        // would never handle the request); a static one takes no request, so only the object
        // is reconciled.
        if let Some(reference) = &source_ref
            && let Some(kind) = reference.kind
            && let Some(source) =
                state::find(&cluster, kind, &reference.namespace, &reference.name, cx)
        {
            if source.suspended {
                skipped.push(format!(
                    "{} {}: its source {} is suspended; resume it first",
                    object.kind.short(),
                    object.key(),
                    reference.label(&object.namespace)
                ));
                continue;
            }
            if !source.reconcilable() {
                source_ref = None;
                job_action = Action::Reconcile;
            }
        }
        let source = source_ref.and_then(|source| {
            let (gvr, resource) = state::resource(&cluster, source.kind?, cx)?;
            Some((
                gvr,
                Target {
                    resource,
                    namespace: source.namespace.clone(),
                    name: source.name.clone(),
                },
            ))
        });
        let check = |gvr: &Gvr, namespace: &str, name: &str, cx: &mut App| {
            let query = AccessQuery::new(verb, gvr, Some(namespace)).name(name);
            let what = format!("{verb} {}.{} in {namespace}", gvr.resource, gvr.group);
            (
                what,
                manager.update(cx, |m, cx| m.can_i(&cluster, query, cx)),
            )
        };
        let mut checks = vec![check(&gvr, &object.namespace, &object.name, cx)];
        if let Some((gvr, source)) = &source {
            checks.push(check(gvr, &source.namespace, &source.name, cx));
        }
        jobs.push(Job {
            label: format!("{} {}", object.kind.short(), object.key()),
            action: job_action,
            checks,
            target: Target {
                resource,
                namespace: object.namespace.clone(),
                name: object.name.clone(),
            },
            source: source.map(|(_, t)| t),
        });
    }
    cx.spawn(async move |cx| {
        let mut done = Vec::new();
        let mut failed = skipped;
        'jobs: for job in jobs {
            for (what, check) in job.checks {
                if check.await == Some(false) {
                    failed.push(format!(
                        "{}: Kubernetes RBAC doesn't allow you to {what}",
                        job.label
                    ));
                    continue 'jobs;
                }
            }
            let client = client.clone();
            let task = cx
                .update(|cx| spawn_kube(cx, ops::run(client, job.target, job.source, job.action)));
            match task.await {
                Ok(()) => done.push(job.label),
                Err(err) => failed.push(format!("{}: {}", job.label, describe(&err))),
            }
        }
        cx.update(|cx| {
            if !done.is_empty() {
                let what = if done.len() == 1 {
                    done[0].clone()
                } else {
                    format!("{} objects", done.len())
                };
                NotificationCenter::push(cx, Notification::info(action.done(&what)));
            }
            for failure in failed {
                notify_error(action.label(), failure, cx);
            }
        });
    })
    .detach();
}

/// Opens the YAML editor on `target` (phase 04; the CRD schema comes with it).
pub fn edit_yaml(target: ResourceRef, window: &mut Window, cx: &mut App) {
    window.dispatch_action(
        Box::new(OpenView(ViewRequest::for_resource(ViewKind::Yaml, target))),
        cx,
    );
}

/// Opens an object's tab.
pub fn open_object(target: ResourceRef, window: &mut Window, cx: &mut App) {
    window.dispatch_action(
        Box::new(OpenView(ViewRequest::for_resource(
            ViewKind::Custom(views::object::VIEW_KIND.into()),
            target,
        ))),
        cx,
    );
}

/// Opens a category's list (`namespace`: filtered to it).
pub fn open_category(
    cluster: &ClusterId,
    category: Category,
    namespace: Option<String>,
    window: &mut Window,
    cx: &mut App,
) {
    window.dispatch_action(
        Box::new(OpenView(ViewRequest::for_resource(
            ViewKind::Custom(category.view_id().into()),
            ResourceRef::list(cluster.clone(), Gvr::new("", "", ""), namespace),
        ))),
        cx,
    );
}

/// The ref of a Flux object on `cluster` (its served version).
pub fn object_ref(cluster: &ClusterId, object: &FluxObject, cx: &App) -> Option<ResourceRef> {
    let gvr = state::gvr(cluster, object.kind, cx)?;
    Some(ResourceRef::object(
        cluster.clone(),
        gvr,
        Some(object.namespace.clone()),
        object.name.clone(),
    ))
}

/// The controller's logs, filtered to lines about the object (phase 05).
pub fn controller_logs(target: &ResourceRef, window: &mut Window, cx: &mut App) {
    let Some(kind) = kind_of(target) else {
        return;
    };
    let install = Flux::try_global(cx).and_then(|f| f.read(cx).install(&target.cluster));
    let Some(install) = install else {
        NotificationCenter::push(
            cx,
            Notification::error("Flux's controllers weren't found (yet)."),
        );
        return;
    };
    let Some(controller) = install.controller(kind.controller()).cloned() else {
        let message = if install.forbidden {
            format!(
                "You may not list Deployments in {}: Kubyl can't find the {} (needs list on deployments.apps).",
                install.namespace.as_deref().unwrap_or("flux-system"),
                kind.controller()
            )
        } else {
            format!("The {} isn't installed.", kind.controller())
        };
        NotificationCenter::push(cx, Notification::error(message));
        return;
    };
    let logs = ResourceRef::object(
        target.cluster.clone(),
        Gvr::new("apps", "v1", "deployments"),
        Some(controller.namespace.clone()),
        controller.name.clone(),
    );
    kubyl_logs::open_filtered(
        logs,
        controller_query(
            kind,
            target.namespace.as_deref().unwrap_or_default(),
            target.name.as_deref().unwrap_or_default(),
        ),
        true,
        window,
        cx,
    );
}

/// Lines of a controller's (JSON) logs about one object: controller-runtime writes
/// `"Kustomization":{"name":"podinfo","namespace":"flux-demo"}`, and lines with a plain
/// `"name":…,"namespace":…` pair count only with `"controllerKind":"<Kind>"` (one controller
/// reconciles several kinds, which may share names: GitRepository and HelmRepository podinfo).
pub fn controller_query(kind: FluxKind, namespace: &str, name: &str) -> String {
    let name = regex_escape(name);
    let namespace = regex_escape(namespace);
    format!(
        r#""{kind}":\{{"name":"{name}","namespace":"{namespace}"\}}|"controllerKind":"{kind}".*"name":"{name}","namespace":"{namespace}"|{kind}/{namespace}/{name}(?:$|[^-\w.])"#,
        kind = kind.kind()
    )
}

fn regex_escape(text: &str) -> String {
    let mut out = String::new();
    for c in text.chars() {
        if "\\.+*?()|[]{}^$".contains(c) {
            out.push('\\');
        }
        out.push(c);
    }
    out
}

/// A failure as a toast.
pub fn notify_error(what: &str, error: impl std::fmt::Display, cx: &mut App) {
    NotificationCenter::push(cx, Notification::error(format!("{what}: {error}")));
}

fn describe(err: &OpError) -> String {
    match err {
        OpError::Forbidden(message) => format!("permission denied ({message})"),
        other => other.to_string(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use kubyl_flux_core::fixtures;
    use std::sync::Arc;

    #[test]
    fn the_log_filter_matches_the_controllers_lines() {
        let query = controller_query(FluxKind::Kustomization, "flux-demo", "broken");
        let regex = regex::Regex::new(&query).unwrap();
        // kustomize-controller 1.9 (Flux 2.9), JSON logs.
        let line = r#"{"level":"error","ts":"2026-10-07T11:11:55.591Z","msg":"Reconciliation failed after 48.245667ms, next try in 1m0s","controller":"kustomization","controllerGroup":"kustomize.toolkit.fluxcd.io","controllerKind":"Kustomization","Kustomization":{"name":"broken","namespace":"flux-demo"},"namespace":"flux-demo","name":"broken","reconcileID":"9cf44906"}"#;
        assert!(regex.is_match(line));
        let other = line.replace("broken", "podinfo");
        assert!(!regex.is_match(&other));
        // source-controller reconciles GitRepository and HelmRepository podinfo: a line about
        // the HelmRepository doesn't match the GitRepository's filter.
        let git = regex::Regex::new(&controller_query(
            FluxKind::GitRepository,
            "flux-demo",
            "podinfo",
        ))
        .unwrap();
        let helm_repo = r#"{"level":"info","msg":"stored artifact","controller":"helmrepository","controllerGroup":"source.toolkit.fluxcd.io","controllerKind":"HelmRepository","HelmRepository":{"name":"podinfo","namespace":"flux-demo"},"namespace":"flux-demo","name":"podinfo"}"#;
        assert!(!git.is_match(helm_repo));
        assert!(git.is_match(&helm_repo.replace("HelmRepository", "GitRepository")));
        // Lines with only the name pair, from the right controller kind.
        let flat = r#"{"msg":"x","controllerKind":"GitRepository","name":"podinfo","namespace":"flux-demo"}"#;
        assert!(git.is_match(flat));
        assert!(!git.is_match(&flat.replace("GitRepository", "Bucket")));
        // `Kind/namespace/name` doesn't match a longer name.
        assert!(regex.is_match("reconciling Kustomization/flux-demo/broken"));
        assert!(!regex.is_match("reconciling Kustomization/flux-demo/broken-2"));
        // Names with regex characters are escaped.
        assert!(controller_query(FluxKind::HelmRelease, "a", "x.y").contains(r"x\.y"));
    }

    #[test]
    fn deleting_on_prod_lists_what_prune_removes_and_asks_for_the_name() {
        let ks = FluxObject::parse(&Arc::new(fixtures::kustomization_ready())).unwrap();
        let target = ResourceRef::object(
            ClusterId::new("c"),
            Gvr::new("kustomize.toolkit.fluxcd.io", "v1", "kustomizations"),
            Some("flux-demo".into()),
            "podinfo".into(),
        );
        let objects = vec![(target.clone(), ks)];
        let none = BTreeSet::new();
        let spec = confirmation(&objects, Action::Delete, true, &none);
        assert_eq!(spec.typed.as_deref(), Some("podinfo"));
        assert!(spec.danger);
        assert_eq!(spec.lines.len(), 3);
        assert_eq!(
            spec.lines[0].as_ref(),
            "prunes Deployment flux-podinfo/podinfo"
        );
        assert!(
            spec.note
                .unwrap()
                .contains("1 Deployment, 1 Service, 1 HorizontalPodAutoscaler")
        );
        // Off PROD: no typed name.
        assert_eq!(
            confirmation(&objects, Action::Delete, false, &none).typed,
            None
        );
        // Suspending several: one line each.
        let mut two = objects.clone();
        two.push(two[0].clone());
        let spec = confirmation(&two, Action::Suspend, false, &none);
        assert_eq!(spec.title.as_ref(), "Suspend 2 objects?");
        assert!(spec.note.unwrap().contains("until they're resumed"));
        // Entries marked to stay are listed as kept, not pruned.
        let deployment = Entry::parse("flux-podinfo_podinfo_apps_Deployment", "v1").unwrap();
        let kept: BTreeSet<Entry> = [deployment].into();
        let spec = confirmation(&objects, Action::Delete, false, &kept);
        assert_eq!(spec.lines.len(), 3);
        assert!(
            spec.lines
                .iter()
                .all(|l| !l.starts_with("prunes Deployment"))
        );
        assert_eq!(
            spec.lines[2].as_ref(),
            "keeps Deployment flux-podinfo/podinfo (marked to stay)"
        );
        assert!(spec.note.unwrap().contains("1 marked to stay"));
        // Suspended: nothing is pruned, and the dialog says so.
        let mut suspended = fixtures::kustomization_ready();
        suspended["spec"]["suspend"] = true.into();
        let suspended = vec![(
            target.clone(),
            FluxObject::parse(&Arc::new(suspended)).unwrap(),
        )];
        let spec = confirmation(&suspended, Action::Delete, false, &none);
        assert!(spec.lines.iter().all(|l| !l.starts_with("prunes")));
        assert!(spec.note.unwrap().starts_with("It's suspended"));
        // deletionPolicy: Delete with prune off still prunes, cluster-scoped entries too.
        let mut chain = fixtures::kustomization_waiting();
        chain["spec"]["prune"] = false.into();
        chain["spec"]["deletionPolicy"] = "Delete".into();
        chain["status"]["inventory"] = serde_json::json!({"entries": [
            {"id": "_flux-chain__Namespace", "v": "v1"},
            {"id": "_flux-chain-reader_rbac.authorization.k8s.io_ClusterRole", "v": "v1"}]});
        let chain = vec![(target, FluxObject::parse(&Arc::new(chain)).unwrap())];
        let spec = confirmation(&chain, Action::Delete, false, &none);
        let lines: Vec<&str> = spec.lines.iter().map(|l| l.as_ref()).collect();
        assert_eq!(
            lines,
            [
                "prunes Namespace flux-chain",
                "prunes ClusterRole flux-chain-reader"
            ]
        );
        assert!(spec.note.unwrap().contains("1 Namespace, 1 ClusterRole"));
    }
}
