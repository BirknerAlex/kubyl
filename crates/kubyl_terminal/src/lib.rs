//! Exec terminal (board 2 · Live logs, exec shell).
//!
//! - [`grid`]: `alacritty_terminal`-backed terminal state and viewport snapshots (colors,
//!   attributes, scrollback, selection, links, modes), GPUI-free and unit-tested.
//! - [`input`]: keystroke, paste and mouse-report encoding (xterm), also pure.
//! - [`shell`]: shell auto-detection (`/bin/bash` -> `/bin/sh` -> `sh`).
//! - [`exec`]: the kube `exec`/`attach` websocket bridge, ephemeral debug containers and node
//!   shells, off the UI thread.
//! - [`view::TerminalView`]: one session painted on a cell grid; `ViewKind::Terminal` builds
//!   plain exec shells as editor tabs.
//! - [`panel`]: the Terminal panel of the bottom dock (tabs and splits).
//! - [`dialog`]: the Debug Container dialog.
//! - [`settings`]: the `"terminal"` settings section.
//!
//! Depends on `kubyl_logs` for the shared active-sessions registry (see
//! `kubyl_logs::sessions` for why that crate owns it).

pub mod dialog;
pub mod panel;
pub mod view;

pub use kubyl_terminal_core::{exec, grid, input, local, settings, shell};

use gpui::{App, AppContext as _, KeyBinding, Window, actions};
use kubyl_core::actions::{ActivateDockPanel, OpenView};
use kubyl_core::{
    ActionRegistry, ActionSpec, ActiveContext, ChromeRegistry, ClusterCaps, ClusterId, ResourceRef,
    ViewKind, ViewRegistry, ViewRequest,
};
use kubyl_explorer::dialogs::{self, ConfirmSpec};
use kubyl_kube::ConnectionManager;
use kubyl_resources::ResourceSelection;
use kubyl_settings::Settings;

use settings::{OpenIn, TerminalSettings};
use view::{SessionMode, TerminalSpec, TerminalView, notify_error};

actions!(
    terminal,
    [
        /// Opens an exec shell into the selected pod (Terminal panel by default).
        ShowShell,
        /// Opens an exec shell into the selected pod as an editor tab.
        ShowShellInTab,
        /// Attaches to the main process of the selected pod (`kubectl attach`).
        AttachToPod,
        /// Adds an ephemeral debug container to the selected pod and attaches to it.
        DebugContainer,
        /// Opens a shell on the selected node through a privileged pod.
        NodeShell,
        /// Shows the Terminal panel.
        ShowTerminalPanel,
        /// Opens a shell on this machine with the active cluster's context (phase 25).
        OpenLocalShell,
    ]
);

const POD_LIST: &str = "ResourceList && kind == Pod";
const NODE_LIST: &str = "ResourceList && kind == Node";

/// Registers this crate's view, panel, actions and settings.
pub fn init(cx: &mut App) {
    Settings::register::<TerminalSettings>(cx);
    view::init_keys(cx);

    ViewRegistry::register(cx, ViewKind::Terminal, |request, _window, cx| {
        let target = request.target.clone();
        Some(Box::new(
            cx.new(|cx| TerminalView::from_request(target, cx)),
        ))
    });
    ChromeRegistry::add_dock_panel(cx, panel::TerminalDockPanel);

    let pods =
        |target: &ResourceRef, caps: &ClusterCaps| target.gvr.resource == "pods" && !caps.read_only;
    ActionRegistry::register(
        cx,
        ActionSpec::new("Pod: Exec Shell", ShowShell)
            .hint("Shell")
            .bind("s", Some(POD_LIST))
            .available_when(pods),
    );
    ActionRegistry::register(
        cx,
        ActionSpec::new("Pod: Exec Shell in Editor Tab", ShowShellInTab)
            .bind("alt-s", Some(POD_LIST))
            .available_when(pods),
    );
    ActionRegistry::register(
        cx,
        ActionSpec::new("Pod: Attach", AttachToPod)
            .hint("Attach")
            .bind("a", Some(POD_LIST))
            .available_when(pods),
    );
    ActionRegistry::register(
        cx,
        ActionSpec::new("Pod: Debug Container…", DebugContainer)
            .hint("Debug")
            .bind("shift-d", Some(POD_LIST))
            .available_when(pods),
    );
    ActionRegistry::register(
        cx,
        ActionSpec::new("Node: Shell…", NodeShell)
            .hint("Shell")
            .bind("s", Some(NODE_LIST))
            .available_when(|target, caps| target.gvr.resource == "nodes" && !caps.read_only),
    );
    ActionRegistry::register(
        cx,
        ActionSpec::new("View: Terminal Panel", ShowTerminalPanel),
    );
    ActionRegistry::register(
        cx,
        ActionSpec::new(
            "Terminal: Open Local Shell for This Cluster",
            OpenLocalShell,
        ),
    );
    cx.bind_keys([KeyBinding::new("ctrl-`", ShowTerminalPanel, None)]);
    // A crash or kill leaves the folders of open local shells behind.
    std::thread::Builder::new()
        .name("kubyl-local-shell-sweep".into())
        .spawn(local::sweep_stale)
        .ok();

    cx.on_action(|_: &ShowShell, cx| {
        if let Some(target) = selected(cx, "pods") {
            let in_tab = Settings::get::<TerminalSettings>(cx).open_in == OpenIn::Tab;
            open(TerminalSpec::exec(target), in_tab, cx);
        }
    });
    cx.on_action(|_: &ShowShellInTab, cx| {
        if let Some(target) = selected(cx, "pods") {
            open(TerminalSpec::exec(target), true, cx);
        }
    });
    cx.on_action(|_: &AttachToPod, cx| {
        if let Some(target) = selected(cx, "pods") {
            open(
                TerminalSpec {
                    target,
                    container: None,
                    mode: SessionMode::Attach,
                },
                false,
                cx,
            );
        }
    });
    cx.on_action(|_: &DebugContainer, cx| {
        if let Some(target) = selected(cx, "pods") {
            debug_container(target, cx);
        }
    });
    cx.on_action(|_: &NodeShell, cx| {
        if let Some(target) = selected(cx, "nodes") {
            node_shell(target, cx);
        }
    });
    cx.on_action(|_: &OpenLocalShell, cx| {
        let Some(cluster) = ActiveContext::global(cx)
            .cluster
            .as_ref()
            .map(|c| c.id.clone())
        else {
            notify_error(cx, "Select a cluster first.");
            return;
        };
        let namespace = ActiveContext::global(cx)
            .namespace
            .as_ref()
            .map(|n| n.to_string());
        open_local(cluster, namespace, cx);
    });
    cx.on_action(|_: &ShowTerminalPanel, cx| {
        with_window(cx, |window, cx| {
            window.dispatch_action(Box::new(ActivateDockPanel(panel::PANEL_ID.into())), cx)
        })
    });
}

/// Runs `f` in the focused window, after the dispatching window's update (global action
/// handlers run inside it).
fn with_window(cx: &mut App, f: impl FnOnce(&mut Window, &mut App) + 'static) {
    cx.defer(move |cx| {
        let window = cx.active_window().or_else(|| cx.windows().first().copied());
        if let Some(window) = window {
            window.update(cx, |_, window, cx| f(window, cx)).ok();
        }
    });
}

/// The selected `resource` object, unless its cluster is read-only (the key binding runs even
/// where the palette and hint bar hide the action).
fn selected(cx: &mut App, resource: &str) -> Option<ResourceRef> {
    let target = ResourceSelection::global(cx)
        .primary()
        .map(|s| s.target.clone())
        .filter(|t| t.is_object() && t.gvr.resource == resource)?;
    let manager = ConnectionManager::global(cx);
    if manager.read(cx).caps(&target.cluster).read_only {
        let name = manager.read(cx).display_name(&target.cluster);
        notify_error(cx, format!("{name} is read-only."));
        return None;
    }
    Some(target)
}

/// Opens a terminal: in the Terminal panel, or (`in_tab`, exec only) as an editor tab.
pub fn open(spec: TerminalSpec, in_tab: bool, cx: &mut App) {
    with_window(cx, move |window, cx| {
        let exec = matches!(spec.mode, SessionMode::Exec { .. } | SessionMode::Local);
        if in_tab && exec {
            window.dispatch_action(
                Box::new(OpenView(ViewRequest::for_resource(
                    ViewKind::Terminal,
                    spec.target,
                ))),
                cx,
            );
            return;
        }
        let target = spec.target.clone();
        if !panel::open_in_panel(spec, window, cx) {
            // No panel in this window (tests, previews): fall back to an editor tab.
            window.dispatch_action(
                Box::new(OpenView(ViewRequest::for_resource(
                    ViewKind::Terminal,
                    target,
                ))),
                cx,
            );
        }
    });
}

/// What the confirmation of a local shell says and asks for, from the cluster's flags:
/// `None` on a plain cluster (the shell opens with a note in its first line), a warning on a
/// read-only one, the typed cluster name on PROD.
pub(crate) fn local_shell_confirmation(name: &str, caps: &ClusterCaps) -> Option<ConfirmSpec> {
    if !caps.read_only && !caps.production {
        return None;
    }
    let mut spec = ConfirmSpec::new(
        format!("Open a local shell for {name}?"),
        "Open Local Shell",
    );
    let flags = match (caps.production, caps.read_only) {
        (true, true) => "production and read-only",
        (true, false) => "production",
        _ => "read-only",
    };
    spec.note = Some(
        format!(
            "{name} is marked {flags} in Kubyl. A local shell runs the kubectl (and any other \
             tool) on your machine with this cluster's credentials, so Kubyl's read-only and \
             PROD protection do not apply to what you run there."
        )
        .into(),
    );
    spec.danger = true;
    spec.typed = caps.production.then(|| name.to_string());
    Some(spec)
}

/// Opens a shell on this machine with the cluster's context in the Terminal panel (or an
/// editor tab, `terminal.open_in`). Read-only and PROD clusters ask first: a local shell
/// bypasses Kubyl's protection, so PROD needs the typed cluster name.
pub fn open_local(cluster: ClusterId, namespace: Option<String>, cx: &mut App) {
    let manager = ConnectionManager::global(cx);
    let (name, caps) = {
        let manager = manager.read(cx);
        (manager.display_name(&cluster), manager.caps(&cluster))
    };
    let in_tab = Settings::get::<TerminalSettings>(cx).open_in == OpenIn::Tab;
    let spec = TerminalSpec::local(cluster, namespace);
    match local_shell_confirmation(&name, &caps) {
        None => open(spec, in_tab, cx),
        Some(confirmation) => with_window(cx, move |window, cx| {
            dialogs::confirm(
                confirmation,
                move |_, _, cx| open(spec.clone(), in_tab, cx),
                window,
                cx,
            );
        }),
    }
}

fn debug_container(target: ResourceRef, cx: &mut App) {
    let Some(client) = ConnectionManager::global(cx)
        .read(cx)
        .client(&target.cluster)
    else {
        notify_error(cx, "The cluster isn't connected.");
        return;
    };
    let image = Settings::get::<TerminalSettings>(cx).debug_image.clone();
    let (namespace, pod) = (
        target.namespace.clone().unwrap_or_default(),
        target.name.clone().unwrap_or_default(),
    );
    let info = kubyl_core::spawn_kube(cx, async move {
        exec::pod_info(&client, &namespace, &pod).await
    });
    with_window(cx, move |window, cx| {
        let pod = target.clone();
        dialog::debug_container(
            pod,
            image,
            info,
            move |spec, _, cx| {
                open(
                    TerminalSpec {
                        target: target.clone(),
                        container: None,
                        mode: SessionMode::Debug(spec),
                    },
                    false,
                    cx,
                )
            },
            window,
            cx,
        );
    });
}

/// Shows the node-shell confirmation (creates a privileged pod) and, once confirmed, calls
/// `on_confirm`. Shared by the `Node: Shell…` action and [`view::TerminalView`]'s unconfirmed node shell (the Details view's Terminal sub-tab).
///
/// `namespace` is the one the pod will actually be created in (the caller's already-resolved
/// value, not re-read from live settings here): settings can change between opening the tab and
/// confirming, and the dialog must show exactly what `start` uses, not a value that may have
/// since drifted.
pub(crate) fn confirm_node_shell(
    target: ResourceRef,
    namespace: &str,
    on_confirm: impl Fn(dialogs::ConfirmResult, &mut Window, &mut App) + 'static,
    window: &mut Window,
    cx: &mut App,
) {
    let node = target.name.clone().unwrap_or_default();
    let manager = ConnectionManager::global(cx);
    if manager.read(cx).caps(&target.cluster).read_only {
        let name = manager.read(cx).display_name(&target.cluster);
        notify_error(cx, format!("{name} is read-only."));
        return;
    }
    // Opening a shell changes nothing on the node by itself: no typed name, even on PROD.
    let mut spec = ConfirmSpec::new(format!("Open a shell on node {node}?"), "Start Node Shell");
    spec.note = Some(
        format!(
            "Creates a privileged pod in {namespace} (host PID, network and IPC) on {node}, with \
             the node's root filesystem at /host: run `chroot /host` to use it. The pod is \
             deleted when the shell closes, and stops after 12 hours at the latest."
        )
        .into(),
    );
    spec.danger = true;
    dialogs::confirm(spec, on_confirm, window, cx);
}

/// Asks, then opens a node shell. Also used by the Terminal panel's `+` on a node-shell tab.
pub(crate) fn node_shell(target: ResourceRef, cx: &mut App) {
    with_window(cx, move |window, cx| {
        let settings = Settings::get::<TerminalSettings>(cx).clone();
        let namespace = settings.node_shell_namespace.clone();
        confirm_node_shell(
            target.clone(),
            &namespace,
            move |_, _, cx| {
                open(
                    TerminalSpec {
                        target: target.clone(),
                        container: None,
                        mode: SessionMode::NodeShell {
                            image: settings.node_shell_image.clone(),
                            namespace: settings.node_shell_namespace.clone(),
                        },
                    },
                    false,
                    cx,
                )
            },
            window,
            cx,
        );
    });
}

#[cfg(test)]
mod tests {
    use super::*;

    fn caps(read_only: bool, production: bool) -> ClusterCaps {
        ClusterCaps {
            read_only,
            production,
            ..Default::default()
        }
    }

    #[test]
    fn a_local_shell_asks_where_kubyl_protects_the_cluster() {
        // Plain clusters open at once (the shell prints a note).
        assert!(local_shell_confirmation("kind-dev", &caps(false, false)).is_none());
        // Read-only: a warning, no typed name.
        let spec = local_shell_confirmation("kind-dev", &caps(true, false)).unwrap();
        assert!(spec.typed.is_none() && spec.danger);
        assert!(spec.note.unwrap().contains("read-only"));
        // PROD: the cluster's name must be typed.
        let spec = local_shell_confirmation("prod-eu", &caps(false, true)).unwrap();
        assert_eq!(spec.typed.as_deref(), Some("prod-eu"));
        let note = spec.note.unwrap();
        assert!(note.contains("production") && note.contains("do not apply"));
        // Both flags: still the typed name.
        let spec = local_shell_confirmation("prod-eu", &caps(true, true)).unwrap();
        assert_eq!(spec.typed.as_deref(), Some("prod-eu"));
        assert!(spec.note.unwrap().contains("production and read-only"));
    }
}
