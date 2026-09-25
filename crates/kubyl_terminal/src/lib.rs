//! Exec terminal view (board 2 · Live logs, exec shell).
//!
//! - [`grid`]: `alacritty_terminal`-backed terminal state (ANSI parsing, cursor, SGR colors),
//!   pure and unit-tested without a cluster.
//! - [`input`]: keystroke -> PTY bytes encoding, also pure.
//! - [`shell`]: shell auto-detection (`/bin/bash` -> `/bin/sh` -> `sh`).
//! - [`exec`]: the kube `exec`/`attach` websocket bridge, off the UI thread.
//! - [`view::TerminalView`]: `ViewKind::Terminal`, rendered as GPUI text (see the module docs
//!   for what a full glyph-atlas renderer would still need).
//! - [`settings`]: the `"terminal"` settings section.
//!
//! Depends on `kubyl_logs` for the shared active-sessions registry (see
//! `kubyl_logs::sessions` for why that crate owns it).

pub mod exec;
pub mod grid;
pub mod input;
pub mod settings;
pub mod shell;
pub mod view;

use gpui::{App, AppContext as _, Window, actions};
use kubyl_core::actions::OpenView;
use kubyl_core::{ActionRegistry, ActionSpec, ViewKind, ViewRegistry, ViewRequest};
use kubyl_resources::ResourceSelection;
use kubyl_settings::Settings;

use settings::TerminalSettings;
use view::TerminalView;

actions!(
    terminal,
    [
        /// Opens an exec shell into the selected pod.
        ShowShell,
    ]
);

/// Registers this crate's view, action and settings.
pub fn init(cx: &mut App) {
    Settings::register::<TerminalSettings>(cx);

    ViewRegistry::register(cx, ViewKind::Terminal, |request, _window, cx| {
        let target = request.target.clone();
        Some(Box::new(cx.new(|cx| TerminalView::new(target, cx))))
    });

    ActionRegistry::register(
        cx,
        ActionSpec::new("Resource: Exec Shell", ShowShell)
            .hint("Shell")
            .bind("s", Some("ResourceList"))
            .available_when(|target, caps| target.gvr.resource == "pods" && !caps.read_only),
    );

    cx.on_action(|_: &ShowShell, cx| {
        let Some(target) = ResourceSelection::global(cx)
            .primary()
            .map(|s| s.target.clone())
            .filter(|t| t.is_object())
        else {
            return;
        };
        cx.defer(move |cx| {
            let window = cx.active_window().or_else(|| cx.windows().first().copied());
            if let Some(window) = window {
                window
                    .update(cx, |_, window: &mut Window, cx| {
                        window.dispatch_action(
                            Box::new(OpenView(ViewRequest::for_resource(
                                ViewKind::Terminal,
                                target,
                            ))),
                            cx,
                        )
                    })
                    .ok();
            }
        });
    });
}
