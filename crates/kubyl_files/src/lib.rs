//! Pod filesystem browser and transfers (board 9).
//!
//! - [`view::FilesView`]: `ViewKind::Files`, a two-pane commander (this machine · the
//!   container) with drag and drop, preview, edit in place and the transfer queue.
//! - [`remote`]: the container side over exec: capability probe, listings, file operations,
//!   and the debug-container fallback for images without a shell.
//! - [`transfer`], [`queue`]: tar/cat/dd transfers with resume and sha256 verification, run by
//!   the app-wide [`queue::TransferQueue`] (per-pod concurrency, history).
//! - [`listing`], [`entry`], [`mounts`], [`local`]: pure helpers, unit-tested without a cluster.

pub mod dialogs;
pub mod entry;
pub mod listing;
pub mod local;
pub mod mounts;
pub mod queue;
pub mod remote;
pub mod settings;
pub mod transfer;
pub mod view;

use gpui::{App, AppContext as _, Window, actions};
use kubyl_core::actions::OpenView;
use kubyl_core::{
    ActionRegistry, ActionSpec, ClusterCaps, Notification, NotificationCenter, ResourceRef,
    ViewKind, ViewRegistry, ViewRequest,
};
use kubyl_kube::ConnectionManager;
use kubyl_resources::ResourceSelection;

use view::FilesView;

actions!(
    files,
    [
        /// Opens the file browser for the selected pod.
        BrowseFiles,
    ]
);

const POD_LIST: &str = "ResourceList && kind == Pod";

/// Registers this crate's views, actions and chrome contributions.
pub fn init(cx: &mut App) {
    kubyl_settings::Settings::register::<settings::FilesSettings>(cx);
    queue::TransferQueue::install(cx);
    view::init(cx);

    ViewRegistry::register(cx, ViewKind::Files, |request, window, cx| {
        let target = request.target.clone();
        Some(Box::new(cx.new(|cx| FilesView::new(target, window, cx))))
    });

    ActionRegistry::register(
        cx,
        ActionSpec::new("Pod: Browse Files", BrowseFiles)
            .hint("Files")
            .bind("f", Some(POD_LIST))
            .available_when(|target: &ResourceRef, caps: &ClusterCaps| {
                target.gvr.resource == "pods" && !caps.read_only
            }),
    );

    cx.on_action(|_: &BrowseFiles, cx| {
        let Some(target) = ResourceSelection::global(cx)
            .primary()
            .map(|s| s.target.clone())
            .filter(|t| t.is_object() && t.gvr.resource == "pods")
        else {
            return;
        };
        let manager = ConnectionManager::global(cx);
        if manager.read(cx).caps(&target.cluster).read_only {
            let name = manager.read(cx).display_name(&target.cluster);
            NotificationCenter::push(cx, Notification::error(format!("{name} is read-only.")));
            return;
        }
        cx.defer(move |cx| {
            let window = cx.active_window().or_else(|| cx.windows().first().copied());
            if let Some(window) = window {
                window
                    .update(cx, |_, window: &mut Window, cx| {
                        window.dispatch_action(
                            Box::new(OpenView(ViewRequest::for_resource(ViewKind::Files, target))),
                            cx,
                        )
                    })
                    .ok();
            }
        });
    });
}
