//! Kubyl: a native Kubernetes desktop client.

// Don't open a console window next to the app on Windows release builds.
#![cfg_attr(all(windows, not(debug_assertions)), windows_subsystem = "windows")]

mod app;
mod logging;
mod platform;
mod views;
mod workspace;

fn main() {
    // `kubyl mcp-bridge`: an agent started Kubyl's tools over stdio (phase 21). No window, no
    // logging: stdout belongs to the agent.
    if std::env::args().nth(1).as_deref() == Some(kubyl_agent::BRIDGE_ARG) {
        std::process::exit(kubyl_agent::bridge());
    }
    // Before any thread exists. WebKitGTK's DMA-BUF renderer makes the compositor drop the
    // connection ("Missing acquire timeline", e.g. NVIDIA on Wayland) as soon as a web view opens.
    #[cfg(target_os = "linux")]
    if std::env::var_os("WEBKIT_DISABLE_DMABUF_RENDERER").is_none() {
        // SAFETY: nothing else runs yet, so no thread reads the environment concurrently.
        unsafe { std::env::set_var("WEBKIT_DISABLE_DMABUF_RENDERER", "1") };
    }
    #[cfg(target_os = "linux")]
    let xwayland = platform::prefer_xwayland();
    let fd_limit = platform::raise_fd_limit();
    let _log_guard = logging::init();
    tracing::info!(version = env!("CARGO_PKG_VERSION"), "starting Kubyl");
    #[cfg(target_os = "linux")]
    if xwayland {
        tracing::info!("using XWayland so web views can be embedded (KUBYL_WAYLAND=1 opts out)");
    }
    match fd_limit {
        Ok(Some((old, new))) => tracing::info!(old, new, "raised the open-file limit"),
        Ok(None) => {}
        Err(err) => tracing::warn!("couldn't raise the open-file limit: {err}"),
    }

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
            kubyl_kubeconfig::init(cx);
            kubyl_alerts::init(cx);
            kubyl_netflow::init(cx);
            kubyl_prometheus::init(cx);
            kubyl_agent::init(cx);
            kubyl_selfupdate::init(cx);
            kubyl_flux::init(cx);
            kubyl_helm::init(cx);
            kubyl_apps::init(cx);
            kubyl_security::init(cx);

            kubyl_settings::Settings::write_schema(cx);
            app::open_window(cx);
        });
}
