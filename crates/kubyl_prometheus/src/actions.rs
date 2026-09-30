//! Palette actions and the keys of the Prometheus view.

use gpui::{App, Window, actions};
use kubyl_core::{ActionRegistry, ActionSpec, ActiveContext, ClusterId};
use kubyl_resources::ResourceSelection;

use crate::service::PrometheusService;
use crate::view::{self, Tab};

actions!(
    prometheus,
    [
        /// Opens the active cluster's Prometheus.
        ShowPrometheus,
        /// Opens the active cluster's Prometheus query tool.
        ShowPrometheusQuery,
        /// Looks for Prometheus servers in the active cluster again.
        LookAgain,
        /// Runs the query.
        RunQuery,
        /// Takes the highlighted completion in the query box.
        AcceptSuggestion,
        NextSuggestion,
        PreviousSuggestion,
        /// Opens the completions for the query box now (Ctrl+Space).
        TriggerSuggestions,
        /// Closes the completions.
        DismissSuggestions,
    ]
);

pub(crate) fn init(cx: &mut App) {
    for spec in [
        ActionSpec::new("Prometheus: Open", ShowPrometheus),
        ActionSpec::new("Prometheus: Open Query Tool", ShowPrometheusQuery),
        ActionSpec::new("Prometheus: Look for Servers Again", LookAgain),
    ] {
        ActionRegistry::register(cx, spec);
    }
    let input = format!("{} > Input", view::VIEW_CONTEXT);
    cx.bind_keys([
        gpui::KeyBinding::new("secondary-enter", RunQuery, Some(view::VIEW_CONTEXT)),
        gpui::KeyBinding::new("tab", AcceptSuggestion, Some(input.as_str())),
        gpui::KeyBinding::new("down", NextSuggestion, Some(input.as_str())),
        gpui::KeyBinding::new("up", PreviousSuggestion, Some(input.as_str())),
        gpui::KeyBinding::new("escape", DismissSuggestions, Some(input.as_str())),
        gpui::KeyBinding::new("ctrl-space", TriggerSuggestions, Some(input.as_str())),
    ]);
    cx.on_action(|_: &ShowPrometheus, cx| open_tab(None, cx));
    cx.on_action(|_: &ShowPrometheusQuery, cx| open_tab(Some(Tab::Query), cx));
    cx.on_action(|_: &LookAgain, cx| {
        if let (Some(cluster), Some(service)) = (active_cluster(cx), PrometheusService::global(cx))
        {
            service.update(cx, |s, cx| s.redetect(&cluster, cx));
        }
    });
}

fn open_tab(tab: Option<Tab>, cx: &mut App) {
    if let Some(cluster) = active_cluster(cx) {
        with_window(cx, move |window, cx| view::open(&cluster, tab, window, cx));
    }
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
