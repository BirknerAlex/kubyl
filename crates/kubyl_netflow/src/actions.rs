//! Keys of the Network Flows view (in the `ActionRegistry`, so the key-hint bar and the palette
//! show them) and the palette actions that open it.

use gpui::{App, Context, Focusable as _, KeyBinding, Window, actions, prelude::*};
use kubyl_core::actions::OpenView;
use kubyl_core::{
    ActionRegistry, ActionSpec, ActiveContext, ClusterId, Gvr, ResourceRef, ViewKind, ViewRequest,
};
use kubyl_resources::ResourceSelection;

use crate::aggregate::Zoom;
use crate::service::FlowService;
use crate::view::{
    self, GRAPH_CONTEXT, LIST_CONTEXT, NetworkFlowsView, Pending, SelectedPart, Tab, VIEW_CONTEXT,
};

actions!(
    netflow,
    [
        /// Opens the active cluster's network flows.
        ShowNetworkFlows,
        /// Opens the network flows of the active namespace.
        ShowNamespaceFlows,
        /// Opens the network flows of the selected object.
        ShowFlowsForSelection,
        /// Looks for a flow source in the active cluster again.
        LookAgain,
        SelectNext,
        SelectPrevious,
        /// Shows or hides the selected flow's details.
        ToggleDetails,
        /// Pauses or resumes the live table.
        TogglePause,
        /// Filters to the selected flow's connection.
        FilterConnection,
        /// Filters to the selected flow's source.
        FilterSource,
        /// Filters to the selected flow's destination.
        FilterDestination,
        /// Opens the selected flow's pod.
        OpenPod,
        /// Back to the newest flows.
        ShowNewest,
        /// Switches to the topology.
        ShowTopology,
        /// Switches to the flow table.
        ShowTable,
        /// Filters the table to the selected node or edge.
        ShowSelectedFlows,
        ZoomNamespaces,
        ZoomWorkloads,
        FitGraph,
        /// Selects the graph's next connection (blocked ones first).
        NextConnection,
        /// Selects the graph's previous connection.
        PreviousConnection,
        /// Closes the details, clears the graph's selection.
        Dismiss,
        FocusFilter,
        BlurFilter,
        AcceptSuggestion,
        NextSuggestion,
        PreviousSuggestion,
    ]
);

/// Kinds whose flows "Show Flows for Selection" opens.
const FLOW_KINDS: &[&str] = &[
    "pods",
    "deployments",
    "statefulsets",
    "daemonsets",
    "replicasets",
    "jobs",
    "services",
    "namespaces",
];

pub(crate) fn init(cx: &mut App) {
    let keyed: Vec<(ActionSpec, &str, &str)> = vec![
        (
            ActionSpec::new("Network Flows: Details", ToggleDetails).hint("Details"),
            "enter",
            LIST_CONTEXT,
        ),
        (
            ActionSpec::new("Network Flows: Pause or Resume", TogglePause).hint("Pause"),
            "space",
            LIST_CONTEXT,
        ),
        (
            ActionSpec::new("Network Flows: Filter to Connection", FilterConnection)
                .hint("Filter to connection"),
            "c",
            LIST_CONTEXT,
        ),
        (
            ActionSpec::new("Network Flows: Filter to Source", FilterSource)
                .hint("Filter to source"),
            "s",
            LIST_CONTEXT,
        ),
        (
            ActionSpec::new("Network Flows: Filter to Destination", FilterDestination)
                .hint("Filter to destination"),
            "d",
            LIST_CONTEXT,
        ),
        (
            ActionSpec::new("Network Flows: Topology", ShowTopology).hint("Topology"),
            "t",
            LIST_CONTEXT,
        ),
        (
            ActionSpec::new("Network Flows: Open Pod", OpenPod).hint("Open pod"),
            "o",
            LIST_CONTEXT,
        ),
        (
            ActionSpec::new("Network Flows: Newest Flows", ShowNewest).hint("Newest"),
            "g",
            LIST_CONTEXT,
        ),
        (
            ActionSpec::new(
                "Network Flows: Show Flows of the Selection",
                ShowSelectedFlows,
            )
            .hint("Show flows"),
            "enter",
            GRAPH_CONTEXT,
        ),
        (
            ActionSpec::new("Network Flows: Flow Table", ShowTable).hint("Flows"),
            "t",
            GRAPH_CONTEXT,
        ),
        (
            ActionSpec::new("Network Flows: Namespaces", ZoomNamespaces).hint("Namespaces"),
            "n",
            GRAPH_CONTEXT,
        ),
        (
            ActionSpec::new("Network Flows: Workloads", ZoomWorkloads).hint("Workloads"),
            "w",
            GRAPH_CONTEXT,
        ),
        (
            ActionSpec::new("Network Flows: Fit the Graph", FitGraph).hint("Fit"),
            "f",
            GRAPH_CONTEXT,
        ),
        (
            ActionSpec::new("Network Flows: Next Connection", NextConnection),
            "]",
            GRAPH_CONTEXT,
        ),
        (
            ActionSpec::new("Network Flows: Previous Connection", PreviousConnection),
            "[",
            GRAPH_CONTEXT,
        ),
    ];
    for (spec, keys, context) in keyed {
        ActionRegistry::register(cx, spec.bind(keys, Some(context)));
    }
    let lists = format!("{LIST_CONTEXT} || {GRAPH_CONTEXT}");
    let input = format!("{VIEW_CONTEXT} > Input");
    cx.bind_keys([
        KeyBinding::new("j", SelectNext, Some(LIST_CONTEXT)),
        KeyBinding::new("down", SelectNext, Some(LIST_CONTEXT)),
        KeyBinding::new("k", SelectPrevious, Some(LIST_CONTEXT)),
        KeyBinding::new("up", SelectPrevious, Some(LIST_CONTEXT)),
        // The graph: nodes busiest first (connections: `]` and `[`).
        KeyBinding::new("j", SelectNext, Some(GRAPH_CONTEXT)),
        KeyBinding::new("down", SelectNext, Some(GRAPH_CONTEXT)),
        KeyBinding::new("k", SelectPrevious, Some(GRAPH_CONTEXT)),
        KeyBinding::new("up", SelectPrevious, Some(GRAPH_CONTEXT)),
        KeyBinding::new("escape", Dismiss, Some(lists.as_str())),
        KeyBinding::new("/", FocusFilter, Some(lists.as_str())),
        KeyBinding::new("escape", BlurFilter, Some(input.as_str())),
        KeyBinding::new("tab", AcceptSuggestion, Some(input.as_str())),
        KeyBinding::new("down", NextSuggestion, Some(input.as_str())),
        KeyBinding::new("up", PreviousSuggestion, Some(input.as_str())),
    ]);
    for spec in [
        ActionSpec::new("Network Flows: Show Network Flows", ShowNetworkFlows),
        ActionSpec::new(
            "Network Flows: Show Network Flows of the Active Namespace",
            ShowNamespaceFlows,
        ),
        ActionSpec::new("Network Flows: Look for a Flow Source Again", LookAgain),
    ] {
        ActionRegistry::register(cx, spec);
    }
    let mut selection = ActionSpec::new(
        "Network Flows: Show Flows for Selection",
        ShowFlowsForSelection,
    )
    .available_when(|target, _| {
        target.is_object() && FLOW_KINDS.contains(&target.gvr.resource.as_str())
    });
    selection.context = Some("ResourceList".into());
    ActionRegistry::register(cx, selection);

    cx.on_action(|_: &ShowNetworkFlows, cx| {
        if let Some(cluster) = active_cluster(cx) {
            with_window(cx, move |window, cx| {
                view::open(&cluster, None, Pending::default(), window, cx)
            });
        }
    });
    cx.on_action(|_: &ShowNamespaceFlows, cx| {
        let active = ActiveContext::global(cx).clone();
        let Some(cluster) = active.cluster.as_ref().map(|c| c.id.clone()) else {
            return;
        };
        let namespace = active.namespace.as_ref().map(|n| n.to_string());
        with_window(cx, move |window, cx| {
            view::open(
                &cluster,
                namespace.as_deref(),
                Pending::default(),
                window,
                cx,
            )
        });
    });
    cx.on_action(|_: &LookAgain, cx| {
        if let (Some(cluster), Some(service)) = (active_cluster(cx), FlowService::global(cx)) {
            service.update(cx, |s, cx| s.redetect(&cluster, cx));
        }
    });
    cx.on_action(|_: &ShowFlowsForSelection, cx| {
        let Some(target) = ResourceSelection::global(cx)
            .primary()
            .map(|s| s.target.clone())
        else {
            return;
        };
        let Some(query) = query_for(&target) else {
            return;
        };
        let cluster = target.cluster.clone();
        with_window(cx, move |window, cx| {
            view::open(
                &cluster,
                None,
                Pending {
                    tab: Some(Tab::Flows),
                    query: Some(query),
                },
                window,
                cx,
            )
        });
    });
}

/// The filter that shows an object's flows (`pod=payments/checkout-api-…`).
pub fn query_for(target: &ResourceRef) -> Option<String> {
    let name = target.name.as_deref()?;
    let ns = target.namespace.as_deref();
    Some(match (target.gvr.resource.as_str(), ns) {
        ("namespaces", _) => format!("ns={name}"),
        ("pods", Some(ns)) => format!("pod={ns}/{name}"),
        ("services", Some(ns)) => format!("service={ns}/{name}"),
        (_, Some(ns)) => format!("workload={ns}/{name}"),
        _ => return None,
    })
}

fn active_cluster(cx: &App) -> Option<ClusterId> {
    ResourceSelection::global(cx)
        .primary()
        .map(|s| s.target.cluster.clone())
        .or_else(|| {
            ActiveContext::global(cx)
                .cluster
                .as_ref()
                .map(|c| c.id.clone())
        })
}

/// Runs `f` in the focused window (deferred: global handlers run inside the window update).
fn with_window(cx: &mut App, f: impl FnOnce(&mut Window, &mut App) + 'static) {
    cx.defer(move |cx| {
        let window = cx.active_window().or_else(|| cx.windows().first().copied());
        if let Some(window) = window {
            window.update(cx, |_, window, cx| f(window, cx)).ok();
        }
    });
}

fn open_pod(view: &mut NetworkFlowsView, window: &mut Window, cx: &mut Context<NetworkFlowsView>) {
    let Some(flow) = view.selected_flow(cx) else {
        return;
    };
    let pod =
        [&flow.source, &flow.destination]
            .into_iter()
            .find_map(|e| match (&e.namespace, &e.pod) {
                (Some(ns), Some(pod)) if !pod.ends_with('*') => {
                    Some((ns.to_string(), pod.to_string()))
                }
                _ => None,
            });
    if let Some((ns, pod)) = pod {
        let target = ResourceRef::object(
            view.cluster.clone(),
            Gvr::new("", "v1", "pods"),
            Some(ns),
            pod,
        );
        window.dispatch_action(
            Box::new(OpenView(ViewRequest::for_resource(
                ViewKind::Details,
                target,
            ))),
            cx,
        );
    }
}

/// Binds the view's key actions.
pub(crate) fn bind_view_actions(this: gpui::Div, cx: &mut Context<NetworkFlowsView>) -> gpui::Div {
    this.on_action(cx.listener(|view, _: &SelectNext, _, cx| match view.tab {
        Tab::Flows => view.move_selection(1, cx),
        Tab::Topology => view.step_graph(false, 1, cx),
    }))
    .on_action(
        cx.listener(|view, _: &SelectPrevious, _, cx| match view.tab {
            Tab::Flows => view.move_selection(-1, cx),
            Tab::Topology => view.step_graph(false, -1, cx),
        }),
    )
    .on_action(cx.listener(|view, _: &NextConnection, _, cx| view.step_graph(true, 1, cx)))
    .on_action(cx.listener(|view, _: &PreviousConnection, _, cx| view.step_graph(true, -1, cx)))
    .on_action(cx.listener(|view, _: &ToggleDetails, _, cx| {
        let open = !view.details_open || view.selected.is_none();
        if view.selected.is_none() {
            view.move_selection(0, cx);
        }
        view.set_details_open(open, cx);
    }))
    .on_action(cx.listener(|view, _: &TogglePause, _, cx| view.toggle_pause(cx)))
    .on_action(cx.listener(|view, _: &FilterConnection, _, cx| {
        view.filter_to_selected(SelectedPart::Connection, cx)
    }))
    .on_action(cx.listener(|view, _: &FilterSource, _, cx| {
        view.filter_to_selected(SelectedPart::Source, cx)
    }))
    .on_action(cx.listener(|view, _: &FilterDestination, _, cx| {
        view.filter_to_selected(SelectedPart::Destination, cx)
    }))
    .on_action(cx.listener(|view, _: &OpenPod, window, cx| open_pod(view, window, cx)))
    .on_action(cx.listener(|view, _: &ShowNewest, _, cx| view.show_newest(cx)))
    .on_action(
        cx.listener(|view, _: &ShowTopology, window, cx| view.set_tab(Tab::Topology, window, cx)),
    )
    .on_action(cx.listener(|view, _: &ShowTable, window, cx| view.set_tab(Tab::Flows, window, cx)))
    .on_action(
        cx.listener(|view, _: &ShowSelectedFlows, window, cx| {
            view.show_selection_flows(window, cx)
        }),
    )
    .on_action(cx.listener(|view, _: &ZoomNamespaces, _, cx| view.set_zoom(Zoom::Namespaces, cx)))
    .on_action(cx.listener(|view, _: &ZoomWorkloads, _, cx| view.set_zoom(Zoom::Workloads, cx)))
    .on_action(cx.listener(|view, _: &FitGraph, _, cx| view.fit_graph(cx)))
    .on_action(cx.listener(|view, _: &Dismiss, _, cx| {
        if view.topology.selected.is_some() {
            view.topology.selected = None;
        } else {
            view.set_details_open(false, cx);
        }
        cx.notify();
    }))
    .on_action(cx.listener(|view, _: &FocusFilter, window, cx| {
        view.input.read(cx).focus_handle(cx).focus(window, cx);
    }))
    .on_action(cx.listener(|view, _: &BlurFilter, window, cx| view.focus_active(window, cx)))
    .on_action(cx.listener(|view, _: &AcceptSuggestion, window, cx| {
        view.accept_suggestion(window, cx);
    }))
    .on_action(cx.listener(|view, _: &NextSuggestion, window, cx| {
        if view.suggestions.is_empty() {
            view.focus_active(window, cx);
        } else {
            view.suggestion = (view.suggestion + 1).min(view.suggestions.len() - 1);
            cx.notify();
        }
    }))
    .on_action(cx.listener(|view, _: &PreviousSuggestion, _, cx| {
        view.suggestion = view.suggestion.saturating_sub(1);
        cx.notify();
    }))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn object_queries() {
        let cluster = ClusterId::new("c");
        let pod = ResourceRef::object(
            cluster.clone(),
            Gvr::new("", "v1", "pods"),
            Some("payments".into()),
            "checkout-api-1".into(),
        );
        assert_eq!(
            query_for(&pod).as_deref(),
            Some("pod=payments/checkout-api-1")
        );
        let deployment = ResourceRef::object(
            cluster.clone(),
            Gvr::new("apps", "v1", "deployments"),
            Some("payments".into()),
            "checkout-api".into(),
        );
        assert_eq!(
            query_for(&deployment).as_deref(),
            Some("workload=payments/checkout-api")
        );
        let namespace = ResourceRef::object(
            cluster,
            Gvr::new("", "v1", "namespaces"),
            None,
            "payments".into(),
        );
        assert_eq!(query_for(&namespace).as_deref(), Some("ns=payments"));
    }
}
