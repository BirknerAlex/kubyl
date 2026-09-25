//! Argo CD actions: in the palette (`> Argo CD: Sync…`), bound to keys in the Applications
//! view (`ArgoApps`), an application's tab (`ArgoApp`) and the explorer's generic Applications
//! table, and shown in their key-hint bars. They act on the selected Application
//! (`ResourceSelection`); an application's tab handles them for its own app.
//!
//! Every action needs the Applications CRD on the selection's cluster, so nothing Argo-related
//! shows elsewhere; writing ones are hidden on read-only clusters.

use gpui::{App, Window, actions};
use kubyl_core::actions::OpenView;
use kubyl_core::{
    ActionRegistry, ActionSpec, ClusterCaps, Notification, NotificationCenter, ResourceRef,
    ViewKind, ViewRequest,
};
use kubyl_resources::ResourceSelection;

use crate::dialogs;
use crate::model::GROUP;
use crate::ops::PolicyChange;
use crate::run::{self, Op};
use crate::views;

actions!(
    argocd,
    [
        /// Opens the Sync dialog of the selected application.
        Sync,
        /// Asks Argo CD to compare the application again.
        Refresh,
        /// Refreshes and regenerates the manifests (skips the repo server's cache).
        HardRefresh,
        /// Opens the Rollback dialog for the previous deployment.
        Rollback,
        /// Terminates the running sync.
        Terminate,
        /// Turns auto-sync on or off.
        ToggleAutoSync,
        /// Deletes the application (cascading or not).
        Delete,
        /// Opens the application's tab.
        OpenApplication,
        /// Opens the application's history.
        ShowHistory,
        /// Opens the application in the YAML editor.
        EditApplication,
        /// Signs in to Argo CD's API (API mode).
        SignIn,
        /// Signs out of API mode.
        SignOut,
        /// Opens the Argo CD web UI in a web view.
        OpenArgoUi,
        /// Opens the Applications list of the selection's cluster.
        OpenApplications,
    ]
);

/// Key context of the Applications view.
pub const APPS_CONTEXT: &str = "ArgoApps";
/// Key context of an application's tab.
pub const APP_CONTEXT: &str = "ArgoApp";
/// The explorer's generic table of Applications.
const LIST_CONTEXT: &str = "ResourceList && kind == Application";

/// The target is an Application on a cluster that serves them.
pub fn is_app(target: &ResourceRef, caps: &ClusterCaps) -> bool {
    caps.argocd.applications
        && target.gvr.group == GROUP
        && target.gvr.resource == "applications"
        && target.is_object()
}

fn writable(target: &ResourceRef, caps: &ClusterCaps) -> bool {
    is_app(target, caps) && !caps.read_only
}

pub(crate) fn init(cx: &mut App) {
    type Pred = fn(&ResourceRef, &ClusterCaps) -> bool;
    // (spec, keys, availability, also in the explorer's generic table)
    let specs: Vec<(ActionSpec, Option<&str>, Pred, bool)> = vec![
        (
            ActionSpec::new("Argo CD: Sync…", Sync).hint("Sync…"),
            Some("s"),
            writable,
            true,
        ),
        (
            ActionSpec::new("Argo CD: Refresh", Refresh).hint("Refresh"),
            Some("r"),
            writable,
            true,
        ),
        (
            ActionSpec::new("Argo CD: Hard Refresh", HardRefresh).hint("Hard refresh"),
            Some("shift-r"),
            writable,
            true,
        ),
        (
            ActionSpec::new("Argo CD: Rollback…", Rollback).hint("Rollback…"),
            Some("b"),
            writable,
            true,
        ),
        (
            ActionSpec::new("Argo CD: History", ShowHistory).hint("History"),
            Some("h"),
            is_app,
            true,
        ),
        // The generic table has its own Edit YAML (`e`) and Delete (`ctrl-d`).
        (
            ActionSpec::new("Argo CD: Edit Application YAML", EditApplication).hint("Edit YAML"),
            Some("e"),
            is_app,
            false,
        ),
        (
            ActionSpec::new("Argo CD: Delete Application…", Delete).hint("Delete…"),
            Some("ctrl-d"),
            writable,
            false,
        ),
        (
            ActionSpec::new("Argo CD: Terminate Operation", Terminate),
            None,
            writable,
            true,
        ),
        (
            ActionSpec::new("Argo CD: Toggle Auto-Sync", ToggleAutoSync),
            None,
            writable,
            true,
        ),
        (
            ActionSpec::new("Argo CD: Open Application", OpenApplication),
            None,
            is_app,
            true,
        ),
    ];
    for (spec, keys, available, in_list) in specs {
        for context in [APPS_CONTEXT, APP_CONTEXT, LIST_CONTEXT] {
            if context == LIST_CONTEXT && !in_list {
                continue;
            }
            let mut spec = spec.clone().available_when(available);
            match keys {
                Some(keys) => spec = spec.bind(keys, Some(context)),
                // Palette only, in this context.
                None => spec.context = Some(context.into()),
            }
            ActionRegistry::register(cx, spec);
        }
    }
    // Global ones (still need Argo CD on the selection's cluster).
    ActionRegistry::register(
        cx,
        ActionSpec::new("Argo CD: Open Applications", OpenApplications)
            .available_when(|_, caps| caps.argocd.applications),
    );
    ActionRegistry::register(
        cx,
        ActionSpec::new("Argo CD: Sign In (API Mode)…", SignIn)
            .available_when(|_, caps| caps.argocd.any()),
    );
    ActionRegistry::register(
        cx,
        ActionSpec::new("Argo CD: Sign Out", SignOut).available_when(|_, caps| caps.argocd.any()),
    );
    ActionRegistry::register(
        cx,
        ActionSpec::new("Argo CD: Open Argo CD UI", OpenArgoUi)
            .available_when(|_, caps| caps.argocd.any()),
    );

    cx.on_action(|_: &Sync, cx| with_app(cx, dialogs::open_sync));
    cx.on_action(|_: &Refresh, cx| run_on_selection(Op::Refresh { hard: false }, cx));
    cx.on_action(|_: &HardRefresh, cx| run_on_selection(Op::Refresh { hard: true }, cx));
    cx.on_action(|_: &Rollback, cx| {
        with_app(cx, |target, window, cx| {
            dialogs::open_rollback(target, None, window, cx)
        })
    });
    cx.on_action(|_: &Terminate, cx| run_on_selection(Op::Terminate, cx));
    cx.on_action(|_: &ToggleAutoSync, cx| {
        if let Some(target) = selected_app(cx) {
            toggle_auto_sync(target, cx);
        }
    });
    cx.on_action(|_: &Delete, cx| with_app(cx, dialogs::open_delete));
    cx.on_action(|_: &OpenApplication, cx| {
        with_app(cx, |target, window, cx| open_app(target, None, window, cx))
    });
    cx.on_action(|_: &ShowHistory, cx| {
        with_app(cx, |target, window, cx| {
            open_app(target, Some(views::app::Tab::History), window, cx)
        })
    });
    cx.on_action(|_: &EditApplication, cx| {
        with_app(cx, |target, window, cx| {
            window.dispatch_action(
                Box::new(OpenView(ViewRequest::for_resource(ViewKind::Yaml, target))),
                cx,
            )
        })
    });
    cx.on_action(|_: &OpenApplications, cx| {
        let Some(cluster) = selection_cluster(cx) else {
            return;
        };
        with_window(cx, move |window, cx| {
            views::apps::open(&cluster, None, window, cx)
        });
    });
    cx.on_action(|_: &SignIn, cx| {
        let Some(cluster) = selection_cluster(cx) else {
            return;
        };
        with_window(cx, move |window, cx| {
            dialogs::open_sign_in(cluster, window, cx)
        });
    });
    cx.on_action(|_: &SignOut, cx| {
        let Some(cluster) = selection_cluster(cx) else {
            return;
        };
        if let Some(argo) = crate::state::ArgoCd::try_global(cx) {
            argo.update(cx, |argo, cx| argo.sign_out(&cluster, false, cx));
        }
    });
    cx.on_action(|_: &OpenArgoUi, cx| {
        let Some(cluster) = selection_cluster(cx) else {
            return;
        };
        with_window(cx, move |window, cx| open_argo_ui(&cluster, window, cx));
    });
}

/// The selected Application, when the selection is one.
pub fn selected_app(cx: &App) -> Option<ResourceRef> {
    let selection = ResourceSelection::global(cx);
    let primary = selection.primary()?;
    is_app(&primary.target, &selection.caps).then(|| primary.target.clone())
}

fn selection_cluster(cx: &App) -> Option<kubyl_core::ClusterId> {
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

fn with_app(cx: &mut App, f: impl FnOnce(ResourceRef, &mut Window, &mut App) + 'static) {
    let Some(target) = selected_app(cx) else {
        return;
    };
    with_window(cx, move |window, cx| f(target, window, cx));
}

fn run_on_selection(op: Op, cx: &mut App) {
    if let Some(target) = selected_app(cx) {
        run::run(target, op, cx).detach();
    }
}

/// Turns auto-sync on or off for `target` (from its loaded object).
pub fn toggle_auto_sync(target: ResourceRef, cx: &mut App) {
    let Some(app) = run::app_target(&target) else {
        return;
    };
    let current = crate::apps::find_app(&target.cluster, &target.gvr, &app, cx)
        .map(|a| a.policy().auto_sync());
    match current {
        Some(on) => run::run(target, Op::Policy(PolicyChange::AutoSync(!on)), cx).detach(),
        None => {
            NotificationCenter::push(cx, Notification::error("The application isn't loaded yet."))
        }
    }
}

/// Opens an application's tab (on `tab`).
pub fn open_app(
    target: ResourceRef,
    tab: Option<views::app::Tab>,
    window: &mut Window,
    cx: &mut App,
) {
    if let Some(tab) = tab {
        views::app::PendingTab::set(&target, tab, cx);
    }
    window.dispatch_action(
        Box::new(OpenView(ViewRequest::for_resource(
            ViewKind::Custom(views::app::VIEW_KIND.into()),
            target,
        ))),
        cx,
    );
}

/// Opens the Argo CD web UI of the cluster's install in a web view (phase 08).
pub fn open_argo_ui(cluster: &kubyl_core::ClusterId, window: &mut Window, cx: &mut App) {
    let Some(argo) = crate::state::ArgoCd::try_global(cx) else {
        return;
    };
    let install = {
        let argo = argo.read(cx);
        argo.api_install(cluster)
            .or_else(|| argo.install_for(cluster, None))
    };
    let Some(install) = install else {
        NotificationCenter::push(
            cx,
            Notification::error("No Argo CD install found on this cluster."),
        );
        return;
    };
    let Some(server) = install.server.clone() else {
        NotificationCenter::push(
            cx,
            Notification::error(format!(
                "No argocd-server Service in {}.",
                install.namespace
            )),
        );
        return;
    };
    let https = !install.insecure && server.https_port.is_some();
    let Some(port) = (if https {
        server.https_port
    } else {
        server.http_port.or(server.https_port)
    }) else {
        return;
    };
    let target = ResourceRef::object(
        cluster.clone(),
        kubyl_core::Gvr::new("", "v1", "services"),
        Some(install.namespace.clone()),
        server.name.clone(),
    );
    let path = (!install.root_path.is_empty()).then(|| format!("{}/", install.root_path));
    window.dispatch_action(
        Box::new(kubyl_webview::OpenWebView {
            target,
            port,
            path,
            ask: false,
        }),
        cx,
    );
}
