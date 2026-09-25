//! Runs an action on an Application: through the Argo CD API when API mode is connected (Argo
//! CD's RBAC decides), else in Kubernetes mode (the user's Kubernetes RBAC decides). Checks the
//! read-only flag and Kubernetes RBAC before writing, and reports the result as a toast.

use gpui::{App, AsyncApp, Task};
use kubyl_core::{ClusterId, Notification, NotificationCenter, ResourceRef, spawn_kube};
use kubyl_kube::ConnectionManager;
use kubyl_kube::access::AccessQuery;

use crate::api::ApiError;
use crate::ops::{self, AppTarget, Cascade, OpError, PolicyChange, SyncRequest};
use crate::state::{self, ArgoCd};

/// What to do.
#[derive(Clone, Debug, PartialEq)]
pub enum Op {
    Refresh {
        hard: bool,
    },
    Sync(SyncRequest),
    Rollback {
        id: i64,
        prune: bool,
        dry_run: bool,
        /// Turn auto-sync off first (the rollback dialog's checkbox).
        disable_auto_sync: bool,
    },
    Terminate,
    Policy(PolicyChange),
    Delete(Cascade),
}

impl Op {
    /// `Sync`, `Rollback`… for messages.
    pub fn label(&self) -> &'static str {
        match self {
            Op::Refresh { hard: false } => "Refresh",
            Op::Refresh { hard: true } => "Hard refresh",
            Op::Sync(request) if request.dry_run => "Dry run",
            Op::Sync(_) => "Sync",
            Op::Rollback { .. } => "Rollback",
            Op::Terminate => "Terminate",
            Op::Policy(PolicyChange::AutoSync(true)) => "Enable auto-sync",
            Op::Policy(PolicyChange::AutoSync(false)) => "Disable auto-sync",
            Op::Policy(PolicyChange::Prune(_)) => "Prune setting",
            Op::Policy(PolicyChange::SelfHeal(_)) => "Self-heal setting",
            Op::Delete(_) => "Delete",
        }
    }

    /// The Kubernetes verb Kubernetes mode needs.
    fn verb(&self) -> &'static str {
        match self {
            Op::Delete(_) => "delete",
            _ => "patch",
        }
    }

    /// Done message.
    fn done(&self, app: &str) -> String {
        match self {
            Op::Refresh { .. } => format!("Refreshing {app}"),
            Op::Sync(request) if request.dry_run => format!("Dry run of {app} started"),
            Op::Sync(_) => format!("Sync of {app} started"),
            Op::Rollback { dry_run: true, .. } => format!("Rollback dry run of {app} started"),
            Op::Rollback { .. } => format!("Rollback of {app} started"),
            Op::Terminate => format!("Terminating the operation of {app}"),
            Op::Policy(PolicyChange::AutoSync(on)) => {
                format!("Auto-sync {} for {app}", if *on { "on" } else { "off" })
            }
            Op::Policy(PolicyChange::Prune(on)) => {
                format!("Prune {} for {app}", if *on { "on" } else { "off" })
            }
            Op::Policy(PolicyChange::SelfHeal(on)) => {
                format!("Self-heal {} for {app}", if *on { "on" } else { "off" })
            }
            Op::Delete(Cascade::None) => format!("{app} deleted; its resources stay"),
            Op::Delete(_) => format!("Deleting {app} and its resources"),
        }
    }
}

/// The app an object ref points at.
pub fn app_target(target: &ResourceRef) -> Option<AppTarget> {
    Some(AppTarget::new(
        target.namespace.clone()?,
        target.name.clone()?,
    ))
}

/// Whether Kubyl may write to the cluster at all (the read-only flag).
pub fn read_only(cluster: &ClusterId, cx: &App) -> bool {
    ConnectionManager::try_global(cx).is_some_and(|m| m.read(cx).caps(cluster).read_only)
}

/// Runs `op` on the app `target` and toasts the outcome.
pub fn run(target: ResourceRef, op: Op, cx: &mut App) -> Task<Result<(), String>> {
    let Some(app) = app_target(&target) else {
        return Task::ready(Err("not an application".into()));
    };
    let cluster = target.cluster.clone();
    if read_only(&cluster, cx) {
        let message = "This cluster is read-only in Kubyl.".to_string();
        NotificationCenter::push(cx, Notification::error(message.clone()));
        return Task::ready(Err(message));
    }
    let api = ArgoCd::try_global(cx).and_then(|argo| argo.read(cx).api(&cluster));
    // Sync policy is part of the spec: always Kubernetes mode.
    let use_api = api.is_some() && !matches!(op, Op::Policy(_));
    cx.spawn(async move |cx| {
        let result = if use_api {
            run_api(&cluster, &app, op.clone(), api.expect("checked"), cx).await
        } else {
            run_kubernetes(&cluster, &target, &app, op.clone(), cx).await
        };
        cx.update(|cx| match &result {
            Ok(()) => NotificationCenter::push(cx, Notification::info(op.done(&app.name))),
            Err(err) => state::notify_error(&format!("{} of {}", op.label(), app.name), err, cx),
        });
        result
    })
}

async fn run_api(
    cluster: &ClusterId,
    app: &AppTarget,
    op: Op,
    api: crate::api::ArgoApi,
    cx: &mut AsyncApp,
) -> Result<(), String> {
    // Rollback's "turn off auto-sync first" is a spec change: Kubernetes mode.
    if let Op::Rollback {
        disable_auto_sync: true,
        ..
    } = &op
    {
        disable_auto_sync(cluster, app, cx).await?;
    }
    let multi = is_multi_source(cluster, app, cx);
    let app2 = app.clone();
    let task = cx.update(|cx| {
        spawn_kube(cx, async move {
            match op {
                Op::Refresh { hard } => api.refresh(&app2, hard).await,
                Op::Sync(request) => api.sync(&app2, &request, multi).await,
                Op::Rollback {
                    id, prune, dry_run, ..
                } => api.rollback(&app2, id, prune, dry_run).await,
                Op::Terminate => api.terminate(&app2).await,
                Op::Delete(cascade) => api.delete(&app2, cascade).await,
                Op::Policy(_) => Ok(()),
            }
        })
    });
    match task.await {
        Ok(()) => Ok(()),
        Err(ApiError::Unauthorized) => {
            let cluster = cluster.clone();
            cx.update(|cx| {
                if let Some(argo) = ArgoCd::try_global(cx) {
                    argo.update(cx, |argo, cx| argo.unauthorized(&cluster, cx));
                }
            });
            Err(ApiError::Unauthorized.to_string())
        }
        Err(err) => Err(err.to_string()),
    }
}

fn is_multi_source(cluster: &ClusterId, app: &AppTarget, cx: &mut AsyncApp) -> bool {
    cx.update(|cx| {
        let Some((gvr, _)) = state::resource(cluster, "applications", cx) else {
            return false;
        };
        crate::apps::find_app(cluster, &gvr, app, cx).is_some_and(|a| a.spec.is_multi_source())
    })
}

/// Kubernetes RBAC for `verb` on the app, from the cache or the API server.
async fn check_access(
    cluster: &ClusterId,
    target: &ResourceRef,
    verb: &str,
    cx: &mut AsyncApp,
) -> Result<(), String> {
    let Some(manager) = cx.update(|cx| ConnectionManager::try_global(cx)) else {
        return Err("not connected".into());
    };
    let query = AccessQuery::new(verb, &target.gvr, target.namespace.as_deref())
        .name(target.name.as_deref().unwrap_or_default());
    let cluster = cluster.clone();
    let task = cx.update(|cx| manager.update(cx, |m, cx| m.can_i(&cluster, query, cx)));
    match task.await {
        Some(false) => Err(format!(
            "Kubernetes RBAC doesn't allow you to {verb} applications.argoproj.io in {}. Sign in to Argo CD (API mode) to act through Argo CD's own permissions.",
            target.namespace.as_deref().unwrap_or_default()
        )),
        _ => Ok(()),
    }
}

async fn disable_auto_sync(
    cluster: &ClusterId,
    app: &AppTarget,
    cx: &mut AsyncApp,
) -> Result<(), String> {
    let (client, resource) = cx.update(|cx| {
        (
            ConnectionManager::try_global(cx).and_then(|m| m.read(cx).client(cluster)),
            state::resource(cluster, "applications", cx),
        )
    });
    let (Some(client), Some((gvr, resource))) = (client, resource) else {
        return Err("not connected".into());
    };
    let target = ResourceRef::object(
        cluster.clone(),
        gvr,
        Some(app.namespace.clone()),
        app.name.clone(),
    );
    check_access(cluster, &target, "patch", cx).await?;
    let app = app.clone();
    let task = cx.update(|cx| {
        spawn_kube(cx, async move {
            ops::set_policy(client, resource, app, PolicyChange::AutoSync(false)).await
        })
    });
    task.await.map_err(|e| e.to_string())
}

async fn run_kubernetes(
    cluster: &ClusterId,
    target: &ResourceRef,
    app: &AppTarget,
    op: Op,
    cx: &mut AsyncApp,
) -> Result<(), String> {
    let (client, resource, username) = cx.update(|cx| {
        (
            ConnectionManager::try_global(cx).and_then(|m| m.read(cx).client(cluster)),
            state::resource(cluster, "applications", cx),
            state::username(cluster, cx),
        )
    });
    let (Some(client), Some((_, resource))) = (client, resource) else {
        return Err("the cluster isn't connected".into());
    };
    check_access(cluster, target, op.verb(), cx).await?;
    if let Op::Rollback {
        disable_auto_sync: true,
        ..
    } = &op
    {
        disable_auto_sync(cluster, app, cx).await?;
    }
    let app = app.clone();
    let task = cx.update(|cx| {
        spawn_kube(cx, async move {
            match op {
                Op::Refresh { hard } => ops::refresh(client, resource, app, hard).await,
                Op::Sync(request) => {
                    ops::start_operation(client, resource, app, |object| {
                        ops::sync_operation(object, &request, &username)
                    })
                    .await
                }
                Op::Rollback {
                    id, prune, dry_run, ..
                } => {
                    ops::start_operation(client, resource, app, |object| {
                        ops::rollback_operation(object, id, prune, dry_run, &username)
                    })
                    .await
                }
                Op::Terminate => ops::terminate(client, resource, app).await,
                Op::Policy(change) => ops::set_policy(client, resource, app, change).await,
                Op::Delete(cascade) => ops::delete(client, resource, app, cascade).await,
            }
        })
    });
    task.await.map_err(|err: OpError| err.to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn labels_and_messages() {
        assert_eq!(Op::Refresh { hard: true }.label(), "Hard refresh");
        assert_eq!(Op::Delete(Cascade::None).verb(), "delete");
        assert_eq!(Op::Sync(SyncRequest::default()).verb(), "patch");
        assert_eq!(
            Op::Delete(Cascade::None).done("guestbook"),
            "guestbook deleted; its resources stay"
        );
        let dry = Op::Sync(SyncRequest {
            dry_run: true,
            ..Default::default()
        });
        assert_eq!(dry.label(), "Dry run");
    }
}
