//! Pulses (k9s `:pulses`): one tile per main kind (pods, workloads, jobs, nodes, events) with
//! the number of healthy and faulty objects and how both moved over the last minutes. The
//! history is sampled while a Pulses tab is open. `:pulses`, `:pulse` and `:pu` open it, and so
//! does "Open Namespace Pulses" in the right-click menu of a namespace favorite.
//!
//! The counting and the history are in `kubyl_pulses_core`.

pub mod view;

pub use kubyl_pulses_core::{Count, History, Kind, count};

use std::sync::Arc;

use gpui::{App, AppContext as _};
use kubyl_core::{ClusterId, Gvr, ResourceRef, ViewKind, ViewRegistry};
use kubyl_explorer::catalog::{self, NamespaceView};
use kubyl_ui::IconName;

/// The view id of the tab.
pub const VIEW_KIND: &str = "pulses";

/// Registers the view, the `:pulses` / `:pulse` / `:pu` command and the right-click entry of a
/// namespace favorite. There is deliberately no sidebar row.
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
    catalog::register_namespace_view(
        cx,
        NamespaceView {
            label: "Open Namespace Pulses",
            kind: ViewKind::Custom(VIEW_KIND.into()),
        },
    );
    kubyl_palette::register_view(
        cx,
        kubyl_palette::PaletteView {
            name: "pulses",
            aliases: vec!["pulse", "pu"],
            detail: "Healthy and faulty objects over time",
            icon: IconName::Activity,
            kind: ViewKind::Custom(VIEW_KIND.into()),
            visible: Arc::new(|_, _| true),
        },
    );
}

/// The ref the tab is opened for (`namespace: None`: all namespaces).
pub fn target(cluster: &ClusterId, namespace: Option<String>) -> ResourceRef {
    ResourceRef::list(cluster.clone(), Gvr::new("", "", ""), namespace)
}
