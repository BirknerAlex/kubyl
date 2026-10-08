//! Helm (phase 22, board 20; releases from phase 12, board 7).
//!
//! Kubyl is a Helm client through the user's `helm` CLI (3.13+ or 4.x):
//! - [`cli::HelmCli`]: the probed `helm`; [`ops::HelmOps`]: the writes that run.
//! - [`service::Helm`]: the releases of each cluster a view shows (metadata-only watches).
//! - [`releases`]: the release list with its details pane (the Operators tab's "Helm releases"
//!   sub-tab renders it). [`release`]: a release's tab. [`charts`]: the Charts tab.
//! - [`dialogs`]: install, upgrade, rollback, uninstall, repositories, "helm isn't installed".
//! - [`managed`]: "Managed by Helm release X" in other objects' details and the YAML editor.
//!
//! The data and the `helm` command lines live in `kubyl_helm_core`, re-exported here.

pub use kubyl_helm_core::{
    agent, cmd, decode, present, preview, repo, settings as helm_settings, values,
};

pub mod charts;
pub mod cli;
pub mod dialogs;
pub mod managed;
pub mod ops;
pub mod release;
pub mod releases;
pub mod service;
#[cfg(test)]
mod tests;
mod values_editor;
pub mod widgets;

use gpui::{App, Window, actions};
use kubyl_core::{ActionRegistry, ActionSpec, ActiveContext, ChromeRegistry, ClusterId};
use kubyl_helm_core::settings::HelmSettings;
use kubyl_settings::Settings;

pub use service::Helm;

/// Hints of writing actions, hidden on read-only clusters (the Operators tab filters them too).
pub const WRITE_HINTS: &[&str] = &[
    "Upgrade…",
    "Roll back…",
    "Uninstall…",
    "Install chart…",
    "Install…",
];

/// Key hints without the writing ones when the cluster is read-only.
pub fn visible_hints(
    hints: Vec<(gpui::SharedString, gpui::SharedString)>,
    writable: bool,
) -> Vec<(gpui::SharedString, gpui::SharedString)> {
    hints
        .into_iter()
        .filter(|(_, hint)| writable || !WRITE_HINTS.contains(&hint.as_ref()))
        .collect()
}

actions!(
    helm,
    [
        /// Installs a chart into the active cluster.
        InstallChart,
        /// Upgrades a release of the active cluster.
        UpgradeRelease,
        /// Rolls a release of the active cluster back to a revision.
        RollBackRelease,
        /// Uninstalls a release of the active cluster.
        UninstallRelease,
        /// Opens the Helm repositories.
        Repositories,
        /// Opens the Charts tab of the active cluster.
        ShowCharts,
    ]
);

/// Registers this crate's views, actions, settings and services.
///
/// Must run after `kubyl_kube::init`, `kubyl_resources::init` and `kubyl_yaml::init`.
pub fn init(cx: &mut App) {
    Settings::register::<HelmSettings>(cx);
    service::Helm::install(true, cx);
    cli::HelmCli::install(true, cx);
    ops::HelmOps::install(cx);
    releases::init(cx);
    release::init(cx);
    charts::init(cx);
    ChromeRegistry::add_details_section(cx, managed::HelmDetails);
    ChromeRegistry::add_edit_notice(cx, managed::HelmNotice);
    for spec in [
        ActionSpec::new("Helm: Install Chart…", InstallChart),
        ActionSpec::new("Helm: Upgrade Release…", UpgradeRelease),
        ActionSpec::new("Helm: Roll Back Release…", RollBackRelease),
        ActionSpec::new("Helm: Uninstall Release…", UninstallRelease),
        ActionSpec::new("Helm: Repositories…", Repositories),
        ActionSpec::new("Helm: Charts", ShowCharts),
    ] {
        ActionRegistry::register(cx, spec);
    }
    // Fallbacks when no release view has focus: the active cluster, a release picked first.
    cx.on_action(|_: &InstallChart, cx| {
        with_window(cx, |window, cx| {
            dialogs::open_install(dialogs::InstallRequest::default(), window, cx)
        })
    });
    cx.on_action(|_: &Repositories, cx| with_window(cx, dialogs::open_repositories));
    cx.on_action(|_: &ShowCharts, cx| {
        if let Some(cluster) = active_cluster(cx) {
            with_window(cx, move |window, cx| charts::open(&cluster, window, cx));
        }
    });
    cx.on_action(|_: &UpgradeRelease, cx| {
        pick(cx, "Upgrade a Helm release", |cluster, row, window, cx| {
            dialogs::open_upgrade(cluster, row, window, cx)
        })
    });
    cx.on_action(|_: &RollBackRelease, cx| {
        pick(
            cx,
            "Roll back a Helm release",
            |cluster, row, window, cx| dialogs::open_rollback(cluster, row, None, window, cx),
        )
    });
    cx.on_action(|_: &UninstallRelease, cx| {
        pick(cx, "Uninstall a Helm release", dialogs::open_uninstall)
    });
}

fn active_cluster(cx: &App) -> Option<ClusterId> {
    ActiveContext::global(cx)
        .cluster
        .as_ref()
        .map(|c| c.id.clone())
}

fn pick(
    cx: &mut App,
    title: &'static str,
    then: impl Fn(ClusterId, service::ReleaseRow, &mut Window, &mut App) + 'static,
) {
    let Some(cluster) = active_cluster(cx) else {
        kubyl_core::NotificationCenter::push(
            cx,
            kubyl_core::Notification::error("Pick a cluster first."),
        );
        return;
    };
    with_window(cx, move |window, cx| {
        let picked = cluster.clone();
        dialogs::pick::pick_release(
            cluster,
            title,
            move |row, window, cx| then(picked.clone(), row, window, cx),
            window,
            cx,
        )
    });
}

/// Runs `f` in the focused window (deferred: global handlers run inside the window update).
pub(crate) fn with_window(cx: &mut App, f: impl FnOnce(&mut Window, &mut App) + 'static) {
    cx.defer(move |cx| {
        let window = cx.active_window().or_else(|| cx.windows().first().copied());
        if let Some(window) = window {
            window.update(cx, |_, window, cx| f(window, cx)).ok();
        }
    });
}
