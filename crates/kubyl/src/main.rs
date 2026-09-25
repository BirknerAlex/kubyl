//! Kubyl: a native Kubernetes desktop client.

// Don't open a console window next to the app on Windows release builds.
#![cfg_attr(all(windows, not(debug_assertions)), windows_subsystem = "windows")]

mod app;
mod logging;
mod platform;
mod views;
mod workspace;

fn main() {
    let _log_guard = logging::init();
    tracing::info!(version = env!("CARGO_PKG_VERSION"), "starting Kubyl");

    gpui_platform::application()
        .with_assets(kubyl_ui::Assets)
        .run(|cx| {
            platform::init(cx);
            kubyl_core::init(cx);
            kubyl_settings::init(cx);
            kubyl_ui::init(cx);
            app::init(cx);

            // Feature crates. Append-only: each phase adds its line and never edits others.
            kubyl_kube::init(cx);
            kubyl_resources::init(cx);
            kubyl_explorer::init(cx);
            kubyl_palette::init(cx);
            kubyl_keymap::init(cx);
            kubyl_yaml::init(cx);
            kubyl_logs::init(cx);
            kubyl_terminal::init(cx);
            kubyl_portforward::init(cx);
            kubyl_files::init(cx);
            kubyl_metrics::init(cx);
            kubyl_charts::init(cx);
            kubyl_overview::init(cx);
            kubyl_operators::init(cx);
            kubyl_updates::init(cx);
            kubyl_webview::init(cx);
            kubyl_argocd::init(cx);

            kubyl_settings::Settings::write_schema(cx);
            app::open_window(cx);
        });
}
