//! Palette actions, key bindings and opening editors.

use std::path::PathBuf;

use gpui::{App, Window, actions};
use kubyl_core::actions::{EditKubeconfig, NewKubeconfig, OpenView};
use kubyl_core::{
    ActionRegistry, ActionSpec, ActiveContext, Notification, NotificationCenter, ViewRequest,
};
use kubyl_kube::ConnectionManager;

use crate::editor::{
    self, CONTEXT, DRAFT_PREFIX, Revert, Save, ShowForm, ShowYaml, TestAll, TestConnection,
    ToggleSecrets,
};
use crate::state::{Draft, Kubeconfigs};

actions!(
    kubeconfig,
    [
        /// Opens the kubeconfig of the active cluster with its context selected.
        EditCurrentContext,
        /// Tests the active cluster's context step by step.
        TestCurrentContext,
        /// Creates a kubeconfig for a service account of the active cluster.
        NewFromServiceAccount,
        /// Creates a kubeconfig from `aws`, `gcloud` or `az`.
        ImportFromCloud,
    ]
);

pub(crate) fn init(cx: &mut App) {
    cx.on_action(|_: &NewKubeconfig, cx| with_window(cx, crate::wizard::open));
    cx.on_action(|action: &EditKubeconfig, cx| {
        open_editor(action.path.clone(), action.context.clone(), false, cx)
    });
    cx.on_action(|_: &EditCurrentContext, cx| current(false, cx));
    cx.on_action(|_: &TestCurrentContext, cx| current(true, cx));
    cx.on_action(|_: &NewFromServiceAccount, cx| with_window(cx, crate::import::service_account));
    cx.on_action(|_: &ImportFromCloud, cx| with_window(cx, crate::import::cloud));

    for spec in [
        ActionSpec::new("Kubeconfig: New…", NewKubeconfig),
        ActionSpec::new("Kubeconfig: Edit Current Context", EditCurrentContext),
        ActionSpec::new("Kubeconfig: Test Current Context", TestCurrentContext),
        ActionSpec::new(
            "Kubeconfig: New from Service Account…",
            NewFromServiceAccount,
        ),
        ActionSpec::new("Kubeconfig: Import from Cloud CLI…", ImportFromCloud),
    ] {
        ActionRegistry::register(cx, spec);
    }
    let ctx = Some(CONTEXT);
    for spec in [
        ActionSpec::new("Kubeconfig: Save…", Save).bind("secondary-s", ctx),
        ActionSpec::new("Kubeconfig: Test Connection", TestConnection)
            .bind("secondary-shift-t", ctx),
        ActionSpec::new("Kubeconfig: Test All Contexts", TestAll).bind("secondary-enter", ctx),
        ActionSpec::new("Kubeconfig: Show Form", ShowForm).bind("secondary-1", ctx),
        ActionSpec::new("Kubeconfig: Show YAML", ShowYaml).bind("secondary-2", ctx),
        ActionSpec::new("Kubeconfig: Revert Changes", Revert).bind("secondary-alt-z", ctx),
        ActionSpec::new("Kubeconfig: Reveal/Mask Secret Values", ToggleSecrets)
            .bind("secondary-shift-r", ctx),
    ] {
        ActionRegistry::register(cx, spec);
    }
}

/// Runs `f` in the focused window. Deferred: global action handlers run while the dispatching
/// window is being updated.
pub(crate) fn with_window(cx: &mut App, f: impl FnOnce(&mut Window, &mut App) + 'static) {
    cx.defer(move |cx| {
        let window = cx.active_window().or_else(|| cx.windows().first().copied());
        if let Some(window) = window
            && let Err(err) = window.update(cx, |_, window, cx| f(window, cx))
        {
            tracing::warn!("no window for the kubeconfig editor: {err:#}");
        }
    });
}

/// Opens (or focuses) the editor of `path`, optionally selecting a context and testing it.
pub fn open_editor(path: PathBuf, context: Option<String>, test: bool, cx: &mut App) {
    if context.is_some() || test {
        Kubeconfigs::global(cx).update(cx, |g, cx| g.request(path.clone(), context, test, cx));
    }
    with_window(cx, move |window, cx| {
        window.dispatch_action(
            Box::new(OpenView(ViewRequest::for_path(editor::view_kind(), path))),
            cx,
        );
    });
}

/// Opens an editor tab for a new, unsaved document.
pub fn open_draft(draft: Draft, cx: &mut App) {
    let id = Kubeconfigs::global(cx).update(cx, |g, _| g.add_draft(draft));
    open_editor(
        PathBuf::from(format!("{DRAFT_PREFIX}{id}")),
        None,
        false,
        cx,
    );
}

fn current(test: bool, cx: &mut App) {
    let Some(id) = ActiveContext::global(cx)
        .cluster
        .as_ref()
        .map(|c| c.id.clone())
    else {
        NotificationCenter::push(cx, Notification::info("No cluster is active."));
        return;
    };
    let Some(info) =
        ConnectionManager::try_global(cx).and_then(|m| m.read(cx).context(&id).cloned())
    else {
        return;
    };
    open_editor(info.file.clone(), Some(info.context.clone()), test, cx);
}
