//! Pulses (k9s `:pulses`): one tile per main kind (pods, workloads, jobs, nodes, events) with
//! the number of healthy and faulty objects and how both moved over the last minutes. The
//! history is sampled while a Pulses tab is open. `:pulses` and `:pu` open it.
//!
//! The counting and the history are in `kubyl_pulses_core`.

pub mod view;

pub use kubyl_pulses_core::{Count, History, Kind, count};

use std::sync::Arc;

use gpui::{App, AppContext as _, actions};
use kubyl_core::actions::OpenView;
use kubyl_core::{
    ActionRegistry, ActionSpec, ActiveContext, ClusterId, Gvr, Notification, NotificationCenter,
    ResourceRef, ViewKind, ViewRegistry, ViewRequest,
};
use kubyl_explorer::catalog::{self, ViewRow};
use kubyl_ui::IconName;

actions!(
    pulses,
    [
        /// Opens the Pulses view of the active cluster.
        ShowPulses,
    ]
);

/// The view id of the tab.
pub const VIEW_KIND: &str = "pulses";

/// Registers the view, the sidebar row, the `:pulses` command and the action.
pub fn init(cx: &mut App) {
    view::init(cx);
    ViewRegistry::register(
        cx,
        ViewKind::Custom(VIEW_KIND.into()),
        |request, window, cx| {
            let target = request.target.clone()?;
            Some(Box::new(
                cx.new(|cx| view::PulsesView::new(target, window, cx)),
            ))
        },
    );
    catalog::register_view_row(
        cx,
        ViewRow {
            id: "pulses",
            after: "overview",
            label: "Pulses",
            icon: IconName::Activity,
            kind: ViewKind::Custom(VIEW_KIND.into()),
            visible: None,
            badge: None,
        },
    );
    kubyl_palette::register_view(
        cx,
        kubyl_palette::PaletteView {
            name: "pulses",
            aliases: vec!["pu"],
            detail: "Healthy and faulty objects over time",
            icon: IconName::Activity,
            kind: ViewKind::Custom(VIEW_KIND.into()),
            visible: Arc::new(|_, _| true),
        },
    );
    ActionRegistry::register(cx, ActionSpec::new("Pulses: Open Pulses", ShowPulses));
    cx.on_action(|_: &ShowPulses, cx| {
        let active = ActiveContext::global(cx);
        let Some(cluster) = active.cluster.as_ref().map(|c| c.id.clone()) else {
            NotificationCenter::push(cx, Notification::info("Select a cluster first."));
            return;
        };
        let namespace = active.namespace.as_ref().map(|n| n.to_string());
        cx.defer(move |cx| {
            let window = cx.active_window().or_else(|| cx.windows().first().copied());
            if let Some(window) = window {
                window
                    .update(cx, |_, window, cx| {
                        window.dispatch_action(
                            Box::new(OpenView(ViewRequest::for_resource(
                                ViewKind::Custom(VIEW_KIND.into()),
                                target(&cluster, namespace),
                            ))),
                            cx,
                        )
                    })
                    .ok();
            }
        });
    });
}

/// The ref the tab is opened for (`namespace: None`: all namespaces).
pub fn target(cluster: &ClusterId, namespace: Option<String>) -> ResourceRef {
    ResourceRef::list(cluster.clone(), Gvr::new("", "", ""), namespace)
}
