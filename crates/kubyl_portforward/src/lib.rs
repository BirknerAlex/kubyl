//! Port-forward manager (board 2 · Live logs, exec shell, port-forwards).
//!
//! - [`resolve`]: Pod/Service/Deployment/StatefulSet/DaemonSet -> pod + remote port, re-run for
//!   every new local connection so Service forwards survive a pod restart.
//! - [`listener`]: the local TCP listener and the per-connection kube portforward bridge, off
//!   the UI thread.
//! - [`manager::PortForwardManager`]: starts/stops forwards, tracks connection/byte counters,
//!   and reports them through `kubyl_logs`'s shared active-sessions registry.
//! - [`favorites`]: persisted favorite forwards (`state.json`), optionally auto-started when
//!   their cluster connects.
//!
//! Depends on `kubyl_logs` for the shared active-sessions registry (list/stop/status-bar
//! counters), same as `kubyl_terminal`.

pub mod favorites;
pub mod listener;
pub mod manager;
pub mod resolve;

use gpui::{App, actions};
use kubyl_core::{ActionRegistry, ActionSpec, ResourceRef};
use kubyl_kube::ConnectionManager;
use kubyl_resources::ResourceSelection;

use manager::{ForwardId, ForwardSpec, PortForwardManager};
use resolve::{ForwardKind, RemotePort};

actions!(
    portforward,
    [
        /// Starts a forward to the selected Pod/Service/workload on its first port.
        StartForward,
        /// Opens the most recently started forward's local URL in the system browser.
        OpenLastInBrowser,
    ]
);

/// Registers the manager, actions and persisted state section.
pub fn init(cx: &mut App) {
    PortForwardManager::install(cx);

    ActionRegistry::register(
        cx,
        ActionSpec::new("Resource: Port-Forward", StartForward)
            .hint("Forward")
            .bind("shift-f", Some("ResourceList"))
            .available_when(|target, caps| {
                !caps.read_only
                    && matches!(
                        target.gvr.resource.as_str(),
                        "pods" | "services" | "deployments" | "statefulsets" | "daemonsets"
                    )
            }),
    );
    ActionRegistry::register(
        cx,
        ActionSpec::new("Port Forward: Open Last in Browser", OpenLastInBrowser)
            .bind("secondary-alt-o", None),
    );

    cx.on_action(|_: &StartForward, cx| {
        let Some(target) = ResourceSelection::global(cx)
            .primary()
            .map(|s| s.target.clone())
            .filter(ResourceRef::is_object)
        else {
            return;
        };
        cx.defer(move |cx| start_default_forward(target, cx));
    });

    cx.on_action(|_: &OpenLastInBrowser, cx| {
        let manager = PortForwardManager::global(cx);
        if let Some(url) = manager.read(cx).last_url() {
            open::that(url).ok();
        }
    });
}

/// Starts a forward using sensible defaults (no dialog yet — see the phase 05 handoff log):
/// the first container port for a Pod/workload, the first Service port for a Service, and an
/// auto-picked local port.
fn start_default_forward(target: ResourceRef, cx: &mut App) {
    let Some(client) = ConnectionManager::global(cx)
        .read(cx)
        .client(&target.cluster)
    else {
        return;
    };
    let Some(name) = target.name.clone() else {
        return;
    };
    let namespace = target.namespace.clone().unwrap_or_default();
    let (kind, port, label) = match target.gvr.resource.as_str() {
        "pods" => (
            ForwardKind::Pod { pod: name.clone() },
            RemotePort::Container(None),
            format!("pod/{name}"),
        ),
        "services" => (
            ForwardKind::Service {
                service: name.clone(),
            },
            RemotePort::Service(None),
            format!("svc/{name}"),
        ),
        resource @ ("deployments" | "statefulsets" | "daemonsets") => {
            let cluster = client.clone();
            let ns = namespace.clone();
            let resource = resource.to_string();
            let name_for_task = name.clone();
            let task = kubyl_core::spawn_kube(cx, async move {
                resolve::workload_selector(cluster, &ns, &resource, &name_for_task).await
            });
            let namespace_task = namespace.clone();
            let target_cluster = target.cluster.clone();
            cx.spawn(async move |cx| {
                if let Ok(selector) = task.await {
                    cx.update(|cx| {
                        if let Some(client) = ConnectionManager::global(cx)
                            .read(cx)
                            .client(&target_cluster)
                        {
                            PortForwardManager::start(
                                client,
                                ForwardSpec {
                                    cluster: target_cluster,
                                    namespace: namespace_task,
                                    kind: ForwardKind::Workload {
                                        label_selector: selector,
                                    },
                                    port: RemotePort::Container(None),
                                    bind_address: "127.0.0.1".into(),
                                    local_port: 0,
                                    label: format!("{}/{name}", target.gvr.resource),
                                },
                                cx,
                            );
                        }
                    });
                }
            })
            .detach();
            return;
        }
        _ => return,
    };
    PortForwardManager::start(
        client,
        ForwardSpec {
            cluster: target.cluster.clone(),
            namespace,
            kind,
            port,
            bind_address: "127.0.0.1".into(),
            local_port: 0,
            label,
        },
        cx,
    );
}

/// Re-exported for callers that already resolved a target (e.g. a future "New Forward" dialog).
pub fn start_forward(client: kube::Client, spec: ForwardSpec, cx: &mut App) -> ForwardId {
    PortForwardManager::start(client, spec, cx)
}
