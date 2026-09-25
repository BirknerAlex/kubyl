//! The first batch of resource actions: registered in the `ActionRegistry` (palette, key hints,
//! context menus) with availability rules, and handled globally on the current
//! [`ResourceSelection`].
//!
//! - Mutating actions are unavailable on read-only clusters (`ClusterCaps::read_only`).
//! - Destructive ones (delete, kill, drain, undo) need the object's name typed on production
//!   clusters (`ClusterCaps::production`).
//! - RBAC: [`access_for`] maps each action to the `SelfSubjectAccessReview` that decides it;
//!   list views hide hints the user may not run, and handlers refuse them.

use std::sync::Arc;

use gpui::{App, ClipboardItem, SharedString, Window, actions};
use kube::discovery::ApiResource;
use kubyl_core::actions::OpenView;
use kubyl_core::{
    ActionRegistry, ActionSpec, ClusterCaps, ClusterId, Notification, NotificationCenter,
    ResourceRef, ViewKind, ViewRequest, spawn_kube,
};
use kubyl_kube::ConnectionManager;
use kubyl_kube::access::AccessQuery;
use kubyl_resources::ops::{self, DeleteOptions};
use kubyl_resources::{ResourceSelection, Selected, StoreMode, format};
use kubyl_settings::Settings;

use crate::dialogs::{self, ConfirmSpec, DrainDialog};
use crate::favorites::{Favorite, Favorites};
use crate::list::CONTEXT;
use crate::settings::ExplorerSettings;

actions!(
    resource,
    [
        /// `kubectl describe` in a tab.
        Describe,
        /// Opens the details tab of the selection.
        ShowDetails,
        /// Deletes the selection (typed confirmation on PROD).
        Delete,
        /// Deletes pods immediately (grace period 0).
        Kill,
        /// Sets the replica count.
        Scale,
        /// `kubectl rollout restart`.
        RolloutRestart,
        /// `kubectl rollout undo` to a chosen revision.
        RolloutUndo,
        /// Pauses or resumes a Deployment rollout.
        TogglePause,
        Cordon,
        Uncordon,
        /// Cordons the node and evicts its pods (PDB-aware).
        Drain,
        /// Creates a Job from the CronJob now.
        TriggerCronJob,
        /// Suspends or resumes a CronJob.
        ToggleSuspend,
        CopyName,
        CopyYaml,
        OpenInNewTab,
        OpenInSplit,
    ]
);

/// A registered action and what it needs.
struct Def {
    name: &'static str,
    hint: Option<&'static str>,
    keys: &'static str,
    /// Kind in the key context (`Pod`), for keys that differ by kind.
    kind_context: Option<&'static str>,
    /// Plural resource names it applies to; empty = every kind.
    resources: &'static [&'static str],
    mutating: bool,
    /// `(verb, subresource)` checked with a SelfSubjectAccessReview.
    access: Option<(&'static str, Option<&'static str>)>,
    spec: fn(&'static str) -> ActionSpec,
}

const SCALABLE: &[&str] = &["deployments", "statefulsets", "replicasets"];
const RESTARTABLE: &[&str] = &["deployments", "statefulsets", "daemonsets"];

fn defs() -> Vec<Def> {
    vec![
        Def {
            name: "Resource: Describe",
            hint: Some("Describe"),
            keys: "d",
            kind_context: None,
            resources: &[],
            mutating: false,
            access: Some(("get", None)),
            spec: |name| ActionSpec::new(name, Describe),
        },
        Def {
            name: "Resource: Show Details",
            hint: None,
            keys: "shift-enter",
            kind_context: None,
            resources: &[],
            mutating: false,
            access: None,
            spec: |name| ActionSpec::new(name, ShowDetails),
        },
        Def {
            name: "Pod: Kill",
            hint: Some("Kill"),
            keys: "ctrl-k",
            kind_context: Some("Pod"),
            resources: &["pods"],
            mutating: true,
            access: Some(("delete", None)),
            spec: |name| ActionSpec::new(name, Kill),
        },
        Def {
            name: "Resource: Delete…",
            hint: Some("Delete"),
            keys: "ctrl-d",
            kind_context: None,
            resources: &[],
            mutating: true,
            access: Some(("delete", None)),
            spec: |name| ActionSpec::new(name, Delete),
        },
        Def {
            name: "Workload: Scale…",
            hint: Some("Scale"),
            keys: "shift-s",
            kind_context: None,
            resources: SCALABLE,
            mutating: true,
            access: Some(("patch", Some("scale"))),
            spec: |name| ActionSpec::new(name, Scale),
        },
        Def {
            name: "Workload: Rollout Restart",
            hint: Some("Restart"),
            keys: "r",
            kind_context: None,
            resources: RESTARTABLE,
            mutating: true,
            access: Some(("patch", None)),
            spec: |name| ActionSpec::new(name, RolloutRestart),
        },
        Def {
            name: "Workload: Rollout Undo…",
            hint: Some("Undo"),
            keys: "u",
            kind_context: None,
            resources: ops::WITH_HISTORY,
            mutating: true,
            access: Some(("update", None)),
            spec: |name| ActionSpec::new(name, RolloutUndo),
        },
        Def {
            name: "Deployment: Pause/Resume Rollout",
            hint: Some("Pause"),
            keys: "p",
            kind_context: Some("Deployment"),
            resources: &["deployments"],
            mutating: true,
            access: Some(("patch", None)),
            spec: |name| ActionSpec::new(name, TogglePause),
        },
        Def {
            name: "Node: Cordon",
            hint: Some("Cordon"),
            keys: "c",
            kind_context: Some("Node"),
            resources: &["nodes"],
            mutating: true,
            access: Some(("patch", None)),
            spec: |name| ActionSpec::new(name, Cordon),
        },
        Def {
            name: "Node: Uncordon",
            hint: Some("Uncordon"),
            keys: "shift-c",
            kind_context: Some("Node"),
            resources: &["nodes"],
            mutating: true,
            access: Some(("patch", None)),
            spec: |name| ActionSpec::new(name, Uncordon),
        },
        Def {
            name: "Node: Drain…",
            hint: Some("Drain"),
            keys: "shift-d",
            kind_context: Some("Node"),
            resources: &["nodes"],
            mutating: true,
            access: Some(("patch", None)),
            spec: |name| ActionSpec::new(name, Drain),
        },
        Def {
            name: "CronJob: Trigger Now",
            hint: Some("Trigger"),
            keys: "t",
            kind_context: Some("CronJob"),
            resources: &["cronjobs"],
            mutating: true,
            access: Some(("create", None)),
            spec: |name| ActionSpec::new(name, TriggerCronJob),
        },
        Def {
            name: "CronJob: Suspend/Resume",
            hint: Some("Suspend"),
            keys: "shift-t",
            kind_context: Some("CronJob"),
            resources: &["cronjobs"],
            mutating: true,
            access: Some(("patch", None)),
            spec: |name| ActionSpec::new(name, ToggleSuspend),
        },
        Def {
            name: "Resource: Copy Name",
            hint: None,
            keys: "y",
            kind_context: None,
            resources: &[],
            mutating: false,
            access: None,
            spec: |name| ActionSpec::new(name, CopyName),
        },
        Def {
            name: "Resource: Copy YAML",
            hint: None,
            keys: "shift-y",
            kind_context: None,
            resources: &[],
            mutating: false,
            access: Some(("get", None)),
            spec: |name| ActionSpec::new(name, CopyYaml),
        },
        Def {
            name: "Resource: Open in New Tab",
            hint: None,
            keys: "secondary-enter",
            kind_context: None,
            resources: &[],
            mutating: false,
            access: None,
            spec: |name| ActionSpec::new(name, OpenInNewTab),
        },
        Def {
            name: "Resource: Open in Split",
            hint: None,
            keys: "alt-enter",
            kind_context: None,
            resources: &[],
            mutating: false,
            access: None,
            spec: |name| ActionSpec::new(name, OpenInSplit),
        },
    ]
}

pub(crate) fn init(cx: &mut App) {
    for def in defs() {
        let context = match def.kind_context {
            Some(kind) => format!("{CONTEXT} && kind == {kind}"),
            None => CONTEXT.to_string(),
        };
        let resources = def.resources;
        let mutating = def.mutating;
        let mut spec = (def.spec)(def.name)
            .bind(def.keys, Some(&context))
            .available_when(move |target: &ResourceRef, caps: &ClusterCaps| {
                (resources.is_empty() || resources.contains(&target.gvr.resource.as_str()))
                    && !(mutating && caps.read_only)
            });
        if let Some(hint) = def.hint {
            spec = spec.hint(hint);
        }
        ActionRegistry::register(cx, spec);
    }
    cx.on_action(|_: &Describe, cx| {
        open_for_selection(ViewKind::Custom("describe".into()), false, cx)
    });
    cx.on_action(|_: &ShowDetails, cx| open_for_selection(ViewKind::Details, false, cx));
    cx.on_action(|_: &OpenInNewTab, cx| open_for_selection(ViewKind::Details, false, cx));
    cx.on_action(|_: &OpenInSplit, cx| open_for_selection(ViewKind::Details, true, cx));
    cx.on_action(|_: &Delete, cx| delete(false, cx));
    cx.on_action(|_: &Kill, cx| delete(true, cx));
    cx.on_action(|_: &Scale, cx| scale(cx));
    cx.on_action(|_: &RolloutRestart, cx| restart(cx));
    cx.on_action(|_: &RolloutUndo, cx| undo(cx));
    cx.on_action(|_: &TogglePause, cx| toggle_pause(cx));
    cx.on_action(|_: &Cordon, cx| cordon(true, cx));
    cx.on_action(|_: &Uncordon, cx| cordon(false, cx));
    cx.on_action(|_: &Drain, cx| drain(cx));
    cx.on_action(|_: &TriggerCronJob, cx| trigger(cx));
    cx.on_action(|_: &ToggleSuspend, cx| toggle_suspend(cx));
    cx.on_action(|_: &CopyName, cx| copy_name(cx));
    cx.on_action(|_: &CopyYaml, cx| copy_yaml(cx));
}

/// The access check that decides whether the action named `name` may run on `target`.
pub fn access_for(name: &str, target: &ResourceRef) -> Option<AccessQuery> {
    let def = defs().into_iter().find(|d| d.name == name)?;
    let (verb, subresource) = def.access?;
    let (group, resource) = match def.name {
        "CronJob: Trigger Now" => ("batch".to_string(), "jobs".to_string()),
        _ => (target.gvr.group.clone(), target.gvr.resource.clone()),
    };
    let gvr = kubyl_core::Gvr::new(group, target.gvr.version.clone(), resource);
    let mut query = AccessQuery::new(verb, &gvr, target.namespace.as_deref());
    if let Some(sub) = subresource {
        query = query.subresource(sub);
    }
    Some(query)
}

// ----- Helpers -----

/// Runs `f` in the focused window.
///
/// Deferred: global action handlers run while the window that dispatched the action is being
/// updated, and updating it again from there fails ("window not found").
pub(crate) fn with_window(cx: &mut App, f: impl FnOnce(&mut Window, &mut App) + 'static) {
    cx.defer(move |cx| {
        let window = cx.active_window().or_else(|| cx.windows().first().copied());
        if let Some(window) = window
            && let Err(err) = window.update(cx, |_, window, cx| f(window, cx))
        {
            tracing::warn!("no window for the action: {err:#}");
        }
    });
}

fn error(cx: &mut App, message: impl Into<SharedString>) {
    NotificationCenter::push(cx, Notification::error(message));
}

fn success(cx: &mut App, message: impl Into<SharedString>) {
    NotificationCenter::push(cx, Notification::success(message));
}

/// `pod/web-0` or `3 pods`.
fn describe_targets(items: &[Selected]) -> String {
    match items {
        [one] => format!(
            "{}/{}",
            one.kind.to_lowercase(),
            one.target.name.clone().unwrap_or_default()
        ),
        many => format!("{} {}", many.len(), many[0].target.gvr.resource),
    }
}

fn line(item: &Selected) -> SharedString {
    match &item.target.namespace {
        Some(ns) => format!("{ns}/{}", item.target.name.clone().unwrap_or_default()).into(),
        None => item.target.name.clone().unwrap_or_default().into(),
    }
}

/// The selection, if an action may run on it. `mutating` checks the read-only flag; `resources`
/// restricts kinds.
fn targets(cx: &mut App, name: &str, mutating: bool) -> Option<(Vec<Selected>, ClusterCaps)> {
    let selection = ResourceSelection::global(cx).clone();
    let def = defs().into_iter().find(|d| d.name == name)?;
    let items: Vec<Selected> = selection
        .items
        .into_iter()
        .filter(|s| {
            def.resources.is_empty() || def.resources.contains(&s.target.gvr.resource.as_str())
        })
        .collect();
    let first = items.first()?;
    let manager = ConnectionManager::global(cx);
    let caps = manager.read(cx).caps(&first.target.cluster);
    if mutating && caps.read_only {
        error(
            cx,
            format!(
                "{} is read-only.",
                manager.read(cx).display_name(&first.target.cluster)
            ),
        );
        return None;
    }
    if let Some(query) = access_for(name, &first.target)
        && manager.read(cx).cached_can_i(&first.target.cluster, &query) == Some(false)
    {
        error(
            cx,
            format!(
                "You are not allowed to {} {}.",
                query.verb, first.target.gvr.resource
            ),
        );
        return None;
    }
    Some((items, caps))
}

pub(crate) fn client_and_resource(
    cx: &App,
    target: &ResourceRef,
) -> Option<(kube::Client, ApiResource)> {
    let manager = ConnectionManager::global(cx);
    let manager = manager.read(cx);
    let client = manager.client(&target.cluster)?;
    let discovery = manager.discovery(&target.cluster)?;
    let info = kubyl_resources::store::find_resource(&discovery.resources, &target.gvr)?;
    Some((client, kubyl_resources::store::api_resource(info)))
}

/// Runs `op` for each target on Tokio and reports one toast.
fn run_each<F, Fut>(items: Vec<Selected>, verb: &'static str, cx: &mut App, op: F)
where
    F: Fn(kube::Client, ApiResource, ResourceRef) -> Fut + Send + Sync + 'static,
    Fut: std::future::Future<Output = Result<(), String>> + Send + 'static,
{
    let label = describe_targets(&items);
    let jobs: Vec<_> = items
        .iter()
        .filter_map(|item| {
            let (client, resource) = client_and_resource(cx, &item.target)?;
            Some((client, resource, item.target.clone()))
        })
        .collect();
    if jobs.is_empty() {
        error(cx, "Not connected.");
        return;
    }
    let op = Arc::new(op);
    let task = spawn_kube(cx, async move {
        let mut errors = Vec::new();
        for (client, resource, target) in jobs {
            if let Err(err) = op(client, resource, target.clone()).await {
                errors.push(format!("{}: {err}", target.name.unwrap_or_default()));
            }
        }
        errors
    });
    cx.spawn(async move |cx| {
        let errors = task.await;
        cx.update(|cx| {
            if errors.is_empty() {
                success(cx, format!("{verb} {label}"));
            } else {
                error(cx, errors.join("\n"));
            }
        });
    })
    .detach();
}

/// Typed confirmation on PROD: the object's name, or `delete N` for several.
fn typed_confirmation(caps: &ClusterCaps, items: &[Selected], verb: &str) -> Option<String> {
    caps.production.then(|| match items {
        [one] => one.target.name.clone().unwrap_or_default(),
        many => format!("{verb} {}", many.len()),
    })
}

// ----- Handlers -----

fn open_for_selection(kind: ViewKind, split: bool, cx: &mut App) {
    let Some(primary) = ResourceSelection::global(cx).primary().cloned() else {
        return;
    };
    with_window(cx, move |window, cx| {
        window.dispatch_action(
            Box::new(OpenView(ViewRequest::for_resource(
                kind,
                primary.target.clone(),
            ))),
            cx,
        );
        if split && let Ok(action) = cx.build_action("pane::SplitRight", None) {
            window.dispatch_action(action, cx);
        }
    });
}

fn delete(force: bool, cx: &mut App) {
    let name = if force {
        "Pod: Kill"
    } else {
        "Resource: Delete…"
    };
    let Some((items, caps)) = targets(cx, name, true) else {
        return;
    };
    let verb = if force { "kill" } else { "delete" };
    let mut spec = ConfirmSpec::new(
        format!(
            "{} {}?",
            if force { "Kill" } else { "Delete" },
            describe_targets(&items)
        ),
        if force { "Kill" } else { "Delete" },
    );
    spec.danger = true;
    spec.lines = items.iter().map(line).collect();
    spec.typed = typed_confirmation(&caps, &items, verb);
    if force {
        spec.note = Some("Pods are removed immediately (grace period 0), without waiting for their containers to stop.".into());
    } else {
        spec.grace_period = Some(Settings::get::<ExplorerSettings>(cx).default_grace_period);
    }
    with_window(cx, move |window, cx| {
        dialogs::confirm(
            spec,
            move |result, _, cx| {
                let options = DeleteOptions {
                    grace_period: result.grace_period,
                    force,
                };
                run_each(
                    items.clone(),
                    if force { "Killed" } else { "Deleted" },
                    cx,
                    move |client, resource, target| {
                        ops::delete(
                            client,
                            resource,
                            target.namespace,
                            target.name.unwrap_or_default(),
                            options,
                        )
                    },
                );
            },
            window,
            cx,
        )
    });
}

fn scale(cx: &mut App) {
    let Some((items, _)) = targets(cx, "Workload: Scale…", true) else {
        return;
    };
    let current = items
        .first()
        .and_then(|s| s.object.as_ref())
        .map(|o| format::int_at(o, "/spec/replicas").max(0) as u32);
    let mut spec = ConfirmSpec::new(format!("Scale {}", describe_targets(&items)), "Scale");
    spec.lines = items.iter().map(line).collect();
    spec.number = Some(current.unwrap_or(1));
    with_window(cx, move |window, cx| {
        dialogs::confirm(
            spec,
            move |result, _, cx| {
                let Some(replicas) = result.number else {
                    return;
                };
                run_each(
                    items.clone(),
                    "Scaled",
                    cx,
                    move |client, resource, target| {
                        ops::scale(
                            client,
                            resource,
                            target.namespace,
                            target.name.unwrap_or_default(),
                            replicas,
                        )
                    },
                );
            },
            window,
            cx,
        )
    });
}

fn restart(cx: &mut App) {
    let Some((items, caps)) = targets(cx, "Workload: Rollout Restart", true) else {
        return;
    };
    let mut spec = ConfirmSpec::new(format!("Restart {}?", describe_targets(&items)), "Restart");
    spec.lines = items.iter().map(line).collect();
    spec.note = Some("Pods are replaced one by one following the rollout strategy.".into());
    spec.typed = typed_confirmation(&caps, &items, "restart");
    with_window(cx, move |window, cx| {
        dialogs::confirm(
            spec,
            move |_, _, cx| {
                run_each(
                    items.clone(),
                    "Restarted",
                    cx,
                    |client, resource, target| {
                        ops::rollout_restart(
                            client,
                            resource,
                            target.namespace,
                            target.name.unwrap_or_default(),
                        )
                    },
                );
            },
            window,
            cx,
        )
    });
}

fn undo(cx: &mut App) {
    let Some((items, caps)) = targets(cx, "Workload: Rollout Undo…", true) else {
        return;
    };
    let Some(item) = items.into_iter().next() else {
        return;
    };
    let Some(client) = ConnectionManager::global(cx)
        .read(cx)
        .client(&item.target.cluster)
    else {
        error(cx, "Not connected.");
        return;
    };
    let namespace = item.target.namespace.clone().unwrap_or_default();
    let name = item.target.name.clone().unwrap_or_default();
    let resource = item.target.gvr.resource.clone();
    let label = format!("{}/{name}", item.kind.to_lowercase());
    let load = spawn_kube(cx, {
        let (client, resource, namespace, name) = (
            client.clone(),
            resource.clone(),
            namespace.clone(),
            name.clone(),
        );
        async move { ops::workload_history(client, &resource, namespace, name).await }
    });
    let typed = caps.production.then(|| name.clone());
    with_window(cx, move |window, cx| {
        dialogs::pick_revision(
            format!("Roll back {label}").into(),
            typed,
            load,
            move |revision, _, cx| {
                let task = spawn_kube(cx, {
                    let (client, resource, namespace, name) = (
                        client.clone(),
                        resource.clone(),
                        namespace.clone(),
                        name.clone(),
                    );
                    async move { ops::workload_undo(client, &resource, namespace, name, revision).await }
                });
                let label = label.clone();
                cx.spawn(async move |cx| {
                    let result = task.await;
                    cx.update(|cx| match result {
                        Ok(()) => {
                            success(cx, format!("Rolled back {label} to revision {revision}"))
                        }
                        Err(err) => error(cx, err),
                    });
                })
                .detach();
            },
            window,
            cx,
        )
    });
}

fn toggle_pause(cx: &mut App) {
    let Some((items, _)) = targets(cx, "Deployment: Pause/Resume Rollout", true) else {
        return;
    };
    for item in items {
        let paused = item
            .object
            .as_ref()
            .and_then(|o| o.pointer("/spec/paused"))
            .and_then(|v| v.as_bool())
            .unwrap_or(false);
        let verb = if paused { "Resumed" } else { "Paused" };
        run_each(vec![item], verb, cx, move |client, _, target| {
            ops::set_paused(
                client,
                target.namespace.unwrap_or_default(),
                target.name.unwrap_or_default(),
                !paused,
            )
        });
    }
}

fn cordon(unschedulable: bool, cx: &mut App) {
    let name = if unschedulable {
        "Node: Cordon"
    } else {
        "Node: Uncordon"
    };
    let Some((items, _)) = targets(cx, name, true) else {
        return;
    };
    let verb = if unschedulable {
        "Cordoned"
    } else {
        "Uncordoned"
    };
    run_each(items, verb, cx, move |client, _, target| {
        ops::cordon(client, target.name.unwrap_or_default(), unschedulable)
    });
}

fn drain(cx: &mut App) {
    let Some((items, caps)) = targets(cx, "Node: Drain…", true) else {
        return;
    };
    let Some(item) = items.into_iter().next() else {
        return;
    };
    let node = item.target.name.clone().unwrap_or_default();
    let mut spec = ConfirmSpec::new(format!("Drain node/{node}?"), "Drain");
    spec.danger = true;
    spec.lines = vec![node.clone().into()];
    spec.note = Some(
        "Cordons the node, then evicts its pods through the Eviction API (PodDisruptionBudgets are respected; blocked evictions are retried for up to 10 minutes). DaemonSet and mirror pods stay.".into(),
    );
    spec.typed = caps.production.then(|| node.clone());
    let cluster = item.target.cluster.clone();
    with_window(cx, move |window, cx| {
        dialogs::confirm(
            spec,
            move |_, window, cx| {
                let Some(client) = ConnectionManager::global(cx).read(cx).client(&cluster) else {
                    error(cx, "Not connected.");
                    return;
                };
                let view = DrainDialog::open(node.clone().into(), window, cx);
                let (tx, mut rx) = futures::channel::mpsc::unbounded();
                let task = spawn_kube(
                    cx,
                    ops::drain(
                        client,
                        node.clone(),
                        std::time::Duration::from_secs(600),
                        tx,
                    ),
                );
                let weak = view.downgrade();
                let node = node.clone();
                cx.spawn(async move |cx| {
                    use futures::StreamExt as _;
                    while let Some(progress) = rx.next().await {
                        weak.update(cx, |view, cx| view.update(progress, cx)).ok();
                    }
                    let result = task.await;
                    cx.update(|cx| match result {
                        Ok(_) => success(cx, format!("Drained node/{node}")),
                        Err(err) => error(cx, format!("Drain of node/{node} stopped: {err}")),
                    });
                })
                .detach();
            },
            window,
            cx,
        )
    });
}

fn trigger(cx: &mut App) {
    let Some((items, _)) = targets(cx, "CronJob: Trigger Now", true) else {
        return;
    };
    for item in items {
        let Some(client) = ConnectionManager::global(cx)
            .read(cx)
            .client(&item.target.cluster)
        else {
            continue;
        };
        let task = spawn_kube(
            cx,
            ops::trigger_cronjob(
                client,
                item.target.namespace.clone().unwrap_or_default(),
                item.target.name.clone().unwrap_or_default(),
            ),
        );
        cx.spawn(async move |cx| {
            let result = task.await;
            cx.update(|cx| match result {
                Ok(job) => success(cx, format!("Created job/{job}")),
                Err(err) => error(cx, err),
            });
        })
        .detach();
    }
}

fn toggle_suspend(cx: &mut App) {
    let Some((items, _)) = targets(cx, "CronJob: Suspend/Resume", true) else {
        return;
    };
    for item in items {
        let suspended = item
            .object
            .as_ref()
            .and_then(|o| o.pointer("/spec/suspend"))
            .and_then(|v| v.as_bool())
            .unwrap_or(false);
        let verb = if suspended { "Resumed" } else { "Suspended" };
        run_each(vec![item], verb, cx, move |client, _, target| {
            ops::set_suspended(
                client,
                target.namespace.unwrap_or_default(),
                target.name.unwrap_or_default(),
                !suspended,
            )
        });
    }
}

fn copy_name(cx: &mut App) {
    let selection = ResourceSelection::global(cx).clone();
    let names: Vec<String> = selection
        .items
        .iter()
        .filter_map(|s| s.target.name.clone())
        .collect();
    if names.is_empty() {
        return;
    }
    cx.write_to_clipboard(ClipboardItem::new_string(names.join("\n")));
    NotificationCenter::push(
        cx,
        Notification::info(format!("Copied {}", names.join(", "))),
    );
}

fn copy_yaml(cx: &mut App) {
    let selection = ResourceSelection::global(cx).clone();
    if selection.is_empty() {
        return;
    }
    // Metadata-only rows (server-side tables) need the full object first.
    let mut fetch = Vec::new();
    let mut ready = Vec::new();
    for item in &selection.items {
        let metadata_only = item
            .store
            .as_ref()
            .is_some_and(|s| s.read(cx).key().mode == StoreMode::Metadata);
        match (&item.object, metadata_only) {
            (Some(object), false) => ready.push((**object).clone()),
            _ => {
                if let Some((client, resource)) = client_and_resource(cx, &item.target) {
                    fetch.push((client, resource, item.target.clone()));
                }
            }
        }
    }
    let task = spawn_kube(cx, async move {
        let mut objects = ready;
        for (client, resource, target) in fetch {
            objects.push(
                ops::get_json(
                    client,
                    resource,
                    target.namespace,
                    target.name.unwrap_or_default(),
                )
                .await?,
            );
        }
        Ok::<_, String>(objects)
    });
    cx.spawn(async move |cx| {
        let result = task.await;
        cx.update(|cx| match result {
            Ok(objects) => {
                let yaml: Vec<String> = objects.iter().map(format::to_yaml).collect();
                cx.write_to_clipboard(ClipboardItem::new_string(yaml.join("---\n")));
                NotificationCenter::push(
                    cx,
                    Notification::info(format!("Copied YAML of {} object(s)", objects.len())),
                );
            }
            Err(err) => error(cx, err),
        });
    })
    .detach();
}

/// Adds `namespace` on `cluster` to the favorites.
pub fn add_favorite(cluster: &ClusterId, namespace: &str, cx: &mut App) {
    let manager = ConnectionManager::global(cx);
    let Some(context) = manager.read(cx).context(cluster).cloned() else {
        return;
    };
    let name = manager.read(cx).display_name(cluster);
    let favorite = Favorite::new(&context, namespace);
    let added = Favorites::global(cx).update(cx, |f, cx| f.add(favorite, cx));
    let message = if added {
        format!("Added {namespace} · {name} to favorites")
    } else {
        format!("{namespace} · {name} is already a favorite")
    };
    NotificationCenter::push(cx, Notification::info(message));
}

#[cfg(test)]
mod tests {
    use super::*;
    use kubyl_core::Gvr;

    #[test]
    fn access_queries() {
        let pod = ResourceRef::object(
            ClusterId::new("c"),
            Gvr::new("", "v1", "pods"),
            Some("web".into()),
            "p".into(),
        );
        let query = access_for("Resource: Delete…", &pod).unwrap();
        assert_eq!(
            (query.verb.as_str(), query.resource.as_str()),
            ("delete", "pods")
        );
        assert_eq!(query.namespace.as_deref(), Some("web"));
        let deploy = ResourceRef::object(
            ClusterId::new("c"),
            Gvr::new("apps", "v1", "deployments"),
            Some("web".into()),
            "d".into(),
        );
        let scale = access_for("Workload: Scale…", &deploy).unwrap();
        assert_eq!(scale.subresource.as_deref(), Some("scale"));
        let cron = ResourceRef::object(
            ClusterId::new("c"),
            Gvr::new("batch", "v1", "cronjobs"),
            Some("web".into()),
            "c".into(),
        );
        assert_eq!(
            access_for("CronJob: Trigger Now", &cron).unwrap().resource,
            "jobs"
        );
        assert!(access_for("Resource: Copy Name", &pod).is_none());
    }

    #[gpui::test]
    fn availability_respects_kind_and_read_only(cx: &mut gpui::TestAppContext) {
        cx.update(|cx| {
            kubyl_core::init(cx);
            init(cx);
            let registry = ActionRegistry::global(cx);
            let pod = ResourceRef::object(
                ClusterId::new("c"),
                Gvr::new("", "v1", "pods"),
                Some("web".into()),
                "p".into(),
            );
            let names = |caps: &ClusterCaps| -> Vec<String> {
                registry
                    .available_for(&pod, caps)
                    .map(|s| s.name.to_string())
                    .collect()
            };
            let writable = names(&ClusterCaps::default());
            assert!(writable.contains(&"Pod: Kill".to_string()));
            assert!(writable.contains(&"Resource: Delete…".to_string()));
            assert!(!writable.contains(&"Workload: Scale…".to_string()));
            let read_only = names(&ClusterCaps {
                read_only: true,
                ..Default::default()
            });
            assert!(!read_only.contains(&"Resource: Delete…".to_string()));
            assert!(read_only.contains(&"Resource: Describe".to_string()));
            assert!(read_only.contains(&"Resource: Copy YAML".to_string()));
        });
    }
}
