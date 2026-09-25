//! Port-forwarding (board 2 · Live logs, exec shell, port-forwards).
//!
//! - [`resolve`]: Pod/Service/Deployment/StatefulSet/DaemonSet -> pod + remote port, re-run for
//!   every new local connection so Service forwards survive a pod restart; the port list for
//!   the picker and HTTP detection.
//! - [`listener`]: the local TCP listener and the per-connection kube portforward bridge, off
//!   the UI thread.
//! - [`manager::PortForwardManager`]: starts/stops forwards, tracks their state (listening,
//!   reconnecting, failed), connections and bytes, and shows them in the Active Sessions panel
//!   of `kubyl_logs` with "open in browser", "copy address" and "save" buttons.
//! - [`favorites`]: saved forwards (`state.json`), started when their cluster connects.
//! - [`dialog`]: the Port-Forward and Saved Port-Forwards dialogs.

pub mod dialog;
pub mod favorites;
pub mod listener;
pub mod manager;
pub mod resolve;

use gpui::{App, Window, actions};
use kubyl_core::{
    ActionRegistry, ActionSpec, ClusterId, Gvr, Notification, NotificationCenter, ResourceRef,
};
use kubyl_kube::{ConnectionEvent, ConnectionManager};
use kubyl_resources::ResourceSelection;

use dialog::ForwardChoice;
use favorites::{SavedForward, SavedForwards};
use manager::{ForwardId, ForwardSpec, PortForwardManager, saved_forward};
use resolve::{ForwardKind, RemotePort};

actions!(
    portforward,
    [
        /// Opens the Port-Forward dialog for the selected Pod/Service/workload.
        StartForward,
        /// Forwards the selected Pod/Service/workload's first port to a free local port,
        /// without the dialog.
        QuickForward,
        /// Opens the most recently started forward in the system browser.
        OpenLastInBrowser,
        /// Lists saved forwards (start, auto-start, remove).
        ShowSavedForwards,
    ]
);

const FORWARDABLE: &[&str] = &[
    "pods",
    "services",
    "deployments",
    "statefulsets",
    "daemonsets",
];

/// Registers the manager, saved forwards and actions.
pub fn init(cx: &mut App) {
    PortForwardManager::install(cx);
    SavedForwards::install(cx);

    let available = |target: &ResourceRef, caps: &kubyl_core::ClusterCaps| {
        !caps.read_only && FORWARDABLE.contains(&target.gvr.resource.as_str())
    };
    ActionRegistry::register(
        cx,
        ActionSpec::new("Resource: Port-Forward…", StartForward)
            .hint("Forward")
            .bind("shift-f", Some("ResourceList"))
            .available_when(available),
    );
    ActionRegistry::register(
        cx,
        ActionSpec::new("Resource: Port-Forward First Port", QuickForward)
            .bind("alt-shift-f", Some("ResourceList"))
            .available_when(available),
    );
    ActionRegistry::register(
        cx,
        ActionSpec::new("Port Forward: Open Last in Browser", OpenLastInBrowser)
            .bind("secondary-alt-o", None),
    );
    ActionRegistry::register(
        cx,
        ActionSpec::new("Port Forward: Saved Forwards…", ShowSavedForwards),
    );

    cx.on_action(|_: &StartForward, cx| {
        let Some((target, client)) = selected(cx) else {
            return;
        };
        let namespace = target.namespace.clone().unwrap_or_default();
        let (resource, name) = (
            target.gvr.resource.clone(),
            target.name.clone().unwrap_or_default(),
        );
        let ports = kubyl_core::spawn_kube(cx, async move {
            resolve::list_ports(client, &namespace, &resource, &name).await
        });
        with_window(cx, move |window, cx| {
            let start_target = target.clone();
            dialog::forward(
                target,
                ports,
                move |choice, _, cx| start_target_forward(start_target.clone(), choice, cx),
                window,
                cx,
            );
        });
    });
    cx.on_action(|_: &QuickForward, cx| {
        if let Some((target, _)) = selected(cx) {
            start_target_forward(
                target,
                ForwardChoice {
                    remote_port: None,
                    local_port: 0,
                    bind_address: "127.0.0.1".into(),
                    http: false,
                    https: false,
                    open_browser: false,
                    save: false,
                    auto_start: false,
                },
                cx,
            );
        }
    });
    cx.on_action(|_: &OpenLastInBrowser, cx| {
        let url = PortForwardManager::global(cx).read(cx).last_url();
        match url {
            // `open_url` hands the URL to the OS without waiting (not `open::that`, which can
            // block the UI thread).
            Some(url) => cx.open_url(&url),
            None => NotificationCenter::push(cx, Notification::info("No port-forward is running.")),
        }
    });
    cx.on_action(|_: &ShowSavedForwards, cx| {
        with_window(cx, |window, cx| {
            dialog::saved(|saved, _, cx| start_saved(&saved, cx), window, cx)
        });
    });

    // Saved forwards with auto-start begin when their cluster connects.
    if let Some(manager) = ConnectionManager::try_global(cx) {
        cx.subscribe(&manager, |manager, event: &ConnectionEvent, cx| {
            let ConnectionEvent::StateChanged(cluster) = event else {
                return;
            };
            if !manager.read(cx).state(cluster).is_connected() {
                return;
            }
            let contexts = manager.read(cx).all_contexts().to_vec();
            let saved: Vec<SavedForward> = SavedForwards::global(cx)
                .read(cx)
                .state()
                .auto_start(cluster, &contexts)
                .cloned()
                .collect();
            let forwards = PortForwardManager::global(cx);
            for forward in saved {
                if !forwards.read(cx).is_running(&forward, cx) {
                    start_saved(&forward, cx);
                }
            }
        })
        .detach();
    }
}

/// Runs `f` in the focused window, after the dispatching window's update.
fn with_window(cx: &mut App, f: impl FnOnce(&mut Window, &mut App) + 'static) {
    cx.defer(move |cx| {
        let window = cx.active_window().or_else(|| cx.windows().first().copied());
        if let Some(window) = window {
            window.update(cx, |_, window, cx| f(window, cx)).ok();
        }
    });
}

fn error(cx: &mut App, message: impl Into<gpui::SharedString>) {
    NotificationCenter::push(cx, Notification::error(message));
}

/// The selected forwardable object and its cluster's client, unless the cluster is read-only
/// or not connected (the key binding runs even where the palette hides the action).
fn selected(cx: &mut App) -> Option<(ResourceRef, kube::Client)> {
    let target = ResourceSelection::global(cx)
        .primary()
        .map(|s| s.target.clone())
        .filter(|t| t.is_object() && FORWARDABLE.contains(&t.gvr.resource.as_str()))?;
    let manager = ConnectionManager::global(cx);
    if manager.read(cx).caps(&target.cluster).read_only {
        let name = manager.read(cx).display_name(&target.cluster);
        error(cx, format!("{name} is read-only."));
        return None;
    }
    let Some(client) = manager.read(cx).client(&target.cluster) else {
        error(cx, "The cluster isn't connected.");
        return None;
    };
    Some((target, client))
}

fn gvr_for(resource: &str) -> Gvr {
    match resource {
        "deployments" | "statefulsets" | "daemonsets" => Gvr::new("apps", "v1", resource),
        other => Gvr::new("", "v1", other),
    }
}

/// Starts a saved forward on its (resolved) cluster.
pub fn start_saved(saved: &SavedForward, cx: &mut App) {
    let manager = ConnectionManager::global(cx);
    let Some(context) = saved.resolve(manager.read(cx).all_contexts()).cloned() else {
        error(
            cx,
            format!("The context of {} isn't loaded.", saved.label()),
        );
        return;
    };
    let target = ResourceRef::object(
        context.id.clone(),
        gvr_for(&saved.resource),
        Some(saved.namespace.clone()),
        saved.name.clone(),
    );
    let (http, https) = saved
        .remote_port
        .map(|p| resolve::http_kind(p, None, None))
        .unwrap_or_default();
    start_target_forward(
        target,
        ForwardChoice {
            remote_port: saved.remote_port,
            local_port: saved.local_port,
            bind_address: saved.bind_address.clone(),
            http,
            https,
            open_browser: false,
            save: false,
            auto_start: saved.auto_start,
        },
        cx,
    );
}

/// Starts a forward to `target` with the dialog's choices (resolving a workload's selector
/// first), and saves it when asked.
pub fn start_target_forward(target: ResourceRef, choice: ForwardChoice, cx: &mut App) {
    let Some(client) = ConnectionManager::global(cx)
        .read(cx)
        .client(&target.cluster)
    else {
        error(cx, "The cluster isn't connected.");
        return;
    };
    let Some(name) = target.name.clone() else {
        return;
    };
    let namespace = target.namespace.clone().unwrap_or_default();
    let spec = |kind: ForwardKind, port: RemotePort, cluster: ClusterId| ForwardSpec {
        cluster,
        namespace: namespace.clone(),
        kind,
        port,
        bind_address: choice.bind_address.clone(),
        local_port: choice.local_port,
        target: target.clone(),
        remote_port: choice.remote_port,
        http: choice.http,
        https: choice.https,
        open_browser: choice.open_browser,
    };
    let save = choice.save.then_some(choice.auto_start);
    match target.gvr.resource.as_str() {
        "pods" => {
            let spec = spec(
                ForwardKind::Pod { pod: name },
                RemotePort::Container(choice.remote_port),
                target.cluster.clone(),
            );
            start_and_save(client, spec, save, cx);
        }
        "services" => {
            let spec = spec(
                ForwardKind::Service { service: name },
                RemotePort::Service(choice.remote_port),
                target.cluster.clone(),
            );
            start_and_save(client, spec, save, cx);
        }
        resource @ ("deployments" | "statefulsets" | "daemonsets") => {
            let (select_client, ns, resource) =
                (client.clone(), namespace.clone(), resource.to_string());
            let task = kubyl_core::spawn_kube(cx, async move {
                resolve::workload_selector(select_client, &ns, &resource, &name).await
            });
            let cluster = target.cluster.clone();
            let base = spec(
                ForwardKind::Workload {
                    label_selector: String::new(),
                },
                RemotePort::Container(choice.remote_port),
                cluster,
            );
            cx.spawn(async move |cx| match task.await {
                Ok(label_selector) => cx.update(|cx| {
                    let spec = ForwardSpec {
                        kind: ForwardKind::Workload { label_selector },
                        ..base
                    };
                    start_and_save(client, spec, save, cx);
                }),
                Err(err) => cx.update(|cx| error(cx, format!("Port-forward failed: {err:#}"))),
            })
            .detach();
        }
        other => error(cx, format!("Port-forwarding isn't supported for {other}.")),
    }
}

/// Starts `spec`; `save`: also save it (the value is its auto-start flag).
fn start_and_save(
    client: kube::Client,
    spec: ForwardSpec,
    save: Option<bool>,
    cx: &mut App,
) -> ForwardId {
    if let Some(auto_start) = save {
        let mut saved = saved_forward(&spec, cx);
        saved.auto_start = auto_start;
        SavedForwards::global(cx).update(cx, |this, cx| this.update_state(cx, |s| s.upsert(saved)));
    }
    PortForwardManager::start(client, spec, cx)
}

/// Starts a forward with an already resolved spec (other crates, tests).
pub fn start_forward(client: kube::Client, spec: ForwardSpec, cx: &mut App) -> ForwardId {
    PortForwardManager::start(client, spec, cx)
}
