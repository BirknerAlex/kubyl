//! Kubyl's cluster UI: the Clusters & kubeconfigs view, the cluster switcher, the OIDC sign-in
//! and exec prompt modals, and a status bar item.

mod clusters_view;
mod exec_prompt;
mod paste;
mod sign_in;
mod status_item;
mod switcher;

use std::path::PathBuf;

use gpui::{App, AppContext as _, PathPromptOptions, Window, actions};
use kubyl_core::actions::{AddKubeconfig, OpenView, SwitchCluster};
use kubyl_core::{
    ActionRegistry, ActionSpec, ChromeRegistry, ClusterId, Notification, NotificationCenter,
    ViewKind, ViewRegistry, ViewRequest,
};

pub use clusters_view::ClustersView;
pub use sign_in::open_sign_in;
pub use switcher::open_switcher;

use crate::ConnectionManager;

actions!(
    kubyl_kube,
    [
        /// Opens the Clusters & kubeconfigs tab.
        OpenClusters,
        /// Opens the "paste kubeconfig YAML" dialog.
        PasteKubeconfig,
        /// Re-reads every kubeconfig source.
        ReloadKubeconfigs,
    ]
);

/// The view kind of the Clusters & kubeconfigs tab.
pub fn clusters_view_kind() -> ViewKind {
    ViewKind::Custom("clusters".into())
}

pub(crate) fn init(cx: &mut App) {
    ViewRegistry::register(cx, clusters_view_kind(), |_, window, cx| {
        Some(Box::new(cx.new(|cx| ClustersView::new(window, cx))))
    });
    ChromeRegistry::add_status_item(cx, status_item::ConnectionStatusItem);

    cx.on_action(|_: &AddKubeconfig, cx| {
        with_active_window(cx, |window, cx| {
            open_clusters(window, cx);
            browse_kubeconfigs(cx);
        })
    });
    cx.on_action(|_: &SwitchCluster, cx| with_active_window(cx, open_switcher));
    cx.on_action(|_: &OpenClusters, cx| with_active_window(cx, open_clusters));
    cx.on_action(|_: &PasteKubeconfig, cx| with_active_window(cx, paste::open_paste_dialog));
    cx.on_action(|_: &ReloadKubeconfigs, cx| {
        ConnectionManager::global(cx).update(cx, |m, cx| m.reload(cx));
    });

    for spec in [
        ActionSpec::new("Clusters: Switch Cluster…", SwitchCluster),
        ActionSpec::new("Clusters: Open Clusters & Kubeconfigs", OpenClusters),
        ActionSpec::new("Clusters: Add Kubeconfig…", AddKubeconfig),
        ActionSpec::new("Clusters: Paste Kubeconfig YAML…", PasteKubeconfig),
        ActionSpec::new("Clusters: Reload Kubeconfigs", ReloadKubeconfigs),
    ] {
        ActionRegistry::register(cx, spec);
    }

    switcher::bind_keys(cx);
    exec_prompt::init(cx);

    // When the user asks for an OIDC cluster that needs a sign-in, show the modal.
    let manager = ConnectionManager::global(cx);
    cx.subscribe(&manager, |_, event: &crate::ConnectionEvent, cx| {
        if let crate::ConnectionEvent::SignInRequested(id) = event {
            let id = id.clone();
            with_active_window(cx, move |window, cx| open_sign_in(id, window, cx));
        }
    })
    .detach();
}

/// Runs `f` in the focused window, if any.
fn with_active_window(cx: &mut App, f: impl FnOnce(&mut Window, &mut App)) {
    let window = cx.active_window().or_else(|| cx.windows().first().copied());
    if let Some(window) = window {
        window.update(cx, |_, window, cx| f(window, cx)).ok();
    }
}

/// Opens (or focuses) the Clusters tab.
pub fn open_clusters(window: &mut Window, cx: &mut App) {
    window.dispatch_action(
        Box::new(OpenView(ViewRequest::new(clusters_view_kind()))),
        cx,
    );
}

/// Shows the file picker and adds the chosen kubeconfig files or folders.
pub fn browse_kubeconfigs(cx: &mut App) {
    let paths = cx.prompt_for_paths(PathPromptOptions {
        files: true,
        directories: true,
        multiple: true,
        prompt: Some("Add".into()),
    });
    cx.spawn(async move |cx| match paths.await {
        Ok(Ok(Some(paths))) if !paths.is_empty() => {
            cx.update(|cx| add_paths(paths, cx));
        }
        Ok(Err(err)) => {
            cx.update(|cx| {
                NotificationCenter::push(
                    cx,
                    Notification::error(format!("Couldn't open the file picker: {err}")),
                )
            });
        }
        _ => {}
    })
    .detach();
}

/// Adds dropped or picked paths as kubeconfig sources.
pub fn add_paths(paths: Vec<PathBuf>, cx: &mut App) {
    ConnectionManager::global(cx).update(cx, |m, cx| m.add_sources(paths, cx));
}

/// Makes `id` the active cluster and connects it. An OIDC cluster without a valid session
/// opens the sign-in modal (through [`crate::ConnectionEvent::SignInRequested`]).
pub fn switch_to(id: &ClusterId, _window: &mut Window, cx: &mut App) {
    ConnectionManager::global(cx).update(cx, |m, cx| m.activate(id, cx));
}
