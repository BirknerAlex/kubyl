//! App-level actions, key bindings, menus and window creation.

use gpui::{
    App, AppContext as _, Bounds, KeyBinding, Menu, MenuItem, SystemMenuType, TitlebarOptions,
    WindowBounds, WindowDecorations, WindowOptions, actions, point, px, size,
};
use kubyl_core::actions::{OpenSettings, ShowNotifications, ToggleCommandPalette};
use kubyl_core::{ActionRegistry, ActionSpec};
use kubyl_settings::State;

use crate::workspace::layout::WorkspaceLayout;
use crate::workspace::{
    About, ActivateNextTab, ActivatePreviousTab, CloseActiveTab, CloseWindow, GoBack, GoForward,
    NewTab, ResetZoom, SplitDown, SplitRight, ToggleBottomDock, ToggleLeftDock, ToggleRightDock,
    ToggleTheme, ToggleZoom, Workspace, ZoomIn, ZoomOut,
};

actions!(kubyl_app, [Quit, NewWindow, OpenRepository]);

/// Reverse-DNS app id (Wayland app_id, bundle identifier).
pub const APP_ID: &str = "io.github.birkneralex.Kubyl";
const REPOSITORY: &str = "https://github.com/BirknerAlex/kubyl";

pub fn init(cx: &mut App) {
    cx.on_action(|_: &Quit, cx| cx.quit());
    cx.on_action(|_: &NewWindow, cx| open_window(cx));
    cx.on_action(|_: &OpenRepository, cx| cx.open_url(REPOSITORY));

    // `secondary` is cmd on macOS and ctrl elsewhere. Actions in the registry are listed by the
    // command palette; the registry also installs their default key binding.
    cx.bind_keys([KeyBinding::new("secondary-k", ToggleCommandPalette, None)]);
    let workspace = Some("Workspace");
    let pane = Some("Pane");
    let specs = [
        (
            ActionSpec::new("App: Quit", Quit),
            Some(("secondary-q", None)),
        ),
        (
            ActionSpec::new("App: New Window", NewWindow),
            Some(("secondary-shift-n", None)),
        ),
        (
            ActionSpec::new("App: Open Settings", OpenSettings),
            Some(("secondary-,", None)),
        ),
        (ActionSpec::new("App: About Kubyl", About), None),
        (
            ActionSpec::new("App: Kubyl on GitHub", OpenRepository),
            None,
        ),
        (
            ActionSpec::new("App: Show Notifications", ShowNotifications),
            None,
        ),
        (
            ActionSpec::new("Theme: Toggle Light/Dark", ToggleTheme),
            None,
        ),
        (
            ActionSpec::new("Workspace: Toggle Sidebar", ToggleLeftDock),
            Some(("secondary-b", workspace)),
        ),
        (
            ActionSpec::new("Workspace: Toggle Details Dock", ToggleRightDock),
            Some(("secondary-r", workspace)),
        ),
        (
            ActionSpec::new("Workspace: Toggle Bottom Dock", ToggleBottomDock),
            Some(("secondary-j", workspace)),
        ),
        (
            ActionSpec::new("Workspace: Close Window", CloseWindow),
            Some(("secondary-shift-w", workspace)),
        ),
        (
            ActionSpec::new("Workspace: Zoom In", ZoomIn),
            Some(("secondary-=", workspace)),
        ),
        (
            ActionSpec::new("Workspace: Zoom Out", ZoomOut),
            Some(("secondary--", workspace)),
        ),
        (
            ActionSpec::new("Workspace: Reset Zoom", ResetZoom),
            Some(("secondary-0", workspace)),
        ),
        (
            ActionSpec::new("Pane: Close Tab", CloseActiveTab),
            Some(("secondary-w", pane)),
        ),
        (
            ActionSpec::new("Pane: New Tab", NewTab),
            Some(("secondary-t", pane)),
        ),
        (
            ActionSpec::new("Pane: Next Tab", ActivateNextTab),
            Some(("ctrl-tab", pane)),
        ),
        (
            ActionSpec::new("Pane: Previous Tab", ActivatePreviousTab),
            Some(("ctrl-shift-tab", pane)),
        ),
        (
            ActionSpec::new("Pane: Split Right", SplitRight),
            Some(("secondary-\\", pane)),
        ),
        (
            ActionSpec::new("Pane: Split Down", SplitDown),
            Some(("secondary-shift-\\", pane)),
        ),
        (
            ActionSpec::new("Pane: Toggle Zoom", ToggleZoom),
            Some(("shift-escape", pane)),
        ),
        (
            ActionSpec::new("Pane: Go Back", GoBack),
            Some(("secondary-[", pane)),
        ),
        (
            ActionSpec::new("Pane: Go Forward", GoForward),
            Some(("secondary-]", pane)),
        ),
    ];
    for (spec, binding) in specs {
        let spec = match binding {
            Some((keys, context)) => spec.bind(keys, context),
            None => spec,
        };
        ActionRegistry::register(cx, spec);
    }
    // Second bindings for the same actions (the registry keeps one per action).
    cx.bind_keys([
        KeyBinding::new("secondary-+", ZoomIn, workspace),
        KeyBinding::new("secondary-shift-]", ActivateNextTab, pane),
        KeyBinding::new("secondary-shift-[", ActivatePreviousTab, pane),
    ]);

    set_menus(cx);
    crate::views::init(cx);

    // Closing the last window quits, on macOS too.
    cx.on_window_closed(|cx, _| {
        if cx.windows().is_empty() {
            cx.quit();
        }
    })
    .detach();
}

fn set_menus(cx: &mut App) {
    cx.set_menus([
        Menu::new("Kubyl").items([
            MenuItem::action("About Kubyl", About),
            MenuItem::separator(),
            MenuItem::action("Settings…", OpenSettings),
            MenuItem::separator(),
            MenuItem::os_submenu("Services", SystemMenuType::Services),
            MenuItem::separator(),
            MenuItem::action("Quit Kubyl", Quit),
        ]),
        Menu::new("File").items([
            MenuItem::action("New Window", NewWindow),
            MenuItem::action("New Tab", NewTab),
            MenuItem::separator(),
            MenuItem::action("Close Tab", CloseActiveTab),
            MenuItem::action("Close Window", CloseWindow),
        ]),
        Menu::new("View").items([
            MenuItem::action("Command Palette…", ToggleCommandPalette),
            MenuItem::separator(),
            MenuItem::action("Toggle Sidebar", ToggleLeftDock),
            MenuItem::action("Toggle Details Dock", ToggleRightDock),
            MenuItem::action("Toggle Bottom Dock", ToggleBottomDock),
            MenuItem::separator(),
            MenuItem::action("Split Right", SplitRight),
            MenuItem::action("Split Down", SplitDown),
            MenuItem::action("Zoom Pane", ToggleZoom),
            MenuItem::separator(),
            MenuItem::action("Zoom In", ZoomIn),
            MenuItem::action("Zoom Out", ZoomOut),
            MenuItem::action("Reset Zoom", ResetZoom),
            MenuItem::separator(),
            MenuItem::action("Toggle Light/Dark Theme", ToggleTheme),
        ]),
        Menu::new("Go").items([
            MenuItem::action("Command Palette…", ToggleCommandPalette),
            MenuItem::separator(),
            MenuItem::action("Back", GoBack),
            MenuItem::action("Forward", GoForward),
        ]),
        Menu::new("Window").items([
            MenuItem::action("Next Tab", ActivateNextTab),
            MenuItem::action("Previous Tab", ActivatePreviousTab),
        ]),
        Menu::new("Help").items([MenuItem::action("Kubyl on GitHub", OpenRepository)]),
    ]);
}

/// Opens a workspace window with the persisted layout.
pub fn open_window(cx: &mut App) {
    let layout = State::get::<WorkspaceLayout>(cx);
    let window_size = layout
        .window_size
        .map(|(w, h)| size(px(w.max(800.0)), px(h.max(500.0))))
        .unwrap_or(size(px(1440.0), px(900.0)));
    let bounds = Bounds::centered(None, window_size, cx);
    let options = WindowOptions {
        window_bounds: Some(if layout.window_maximized {
            WindowBounds::Maximized(bounds)
        } else {
            WindowBounds::Windowed(bounds)
        }),
        titlebar: Some(TitlebarOptions {
            title: Some("Kubyl".into()),
            appears_transparent: true,
            // Vertically centered in the 38px title bar.
            traffic_light_position: Some(point(px(14.0), px(12.0))),
        }),
        // The title bar moves the window itself (see gpui_component::TitleBar).
        app_owns_titlebar_drag: true,
        window_min_size: Some(size(px(800.0), px(500.0))),
        window_decorations: Some(WindowDecorations::Client),
        app_id: Some(APP_ID.to_string()),
        icon: crate::platform::window_icon(),
        ..Default::default()
    };
    let result = cx.open_window(options, |window, cx| {
        let workspace = cx.new(|cx| Workspace::new(layout, window, cx));
        cx.new(|cx| gpui_component::Root::new(workspace, window, cx))
    });
    match result {
        #[cfg(feature = "screenshot")]
        Ok(window) => screenshot::schedule(window.into(), cx),
        #[cfg(not(feature = "screenshot"))]
        Ok(_) => {}
        Err(err) => tracing::error!("failed to open window: {err:#}"),
    }
    cx.activate(true);
}

/// Dev tool for PR screenshots and visual checks (see the `screenshot` feature).
#[cfg(feature = "screenshot")]
mod screenshot {
    use std::time::Duration;

    use gpui::{AnyWindowHandle, App, Window};
    use kubyl_core::{Notification, NotificationCenter};

    use crate::workspace::{About, SplitRight, ToggleBottomDock, ToggleTheme, ToggleZoom};

    fn run_step(step: &str, window: &mut Window, cx: &mut App) {
        match step {
            "split" => window.dispatch_action(Box::new(SplitRight), cx),
            "bottom" => window.dispatch_action(Box::new(ToggleBottomDock), cx),
            "zoom" => window.dispatch_action(Box::new(ToggleZoom), cx),
            "light" => window.dispatch_action(Box::new(ToggleTheme), cx),
            "about" => window.dispatch_action(Box::new(About), cx),
            "toast" => {
                NotificationCenter::push(cx, Notification::success("Connected to kind-kubyl-dev"))
            }
            // `toast=<text>` posts an error notification with that text (no commas).
            step if step.starts_with("toast=") => NotificationCenter::push(
                cx,
                Notification::error(step["toast=".len()..].to_string()),
            ),
            // `palette=:cert` opens the command palette with that query; `action=pane::GoBack`
            // dispatches any action by name.
            // `mouse=640:380` moves the pointer there (hover popups), `click=640:380` clicks,
            // in logical window pixels.
            // `scroll=640:380:-600` scrolls the element under that point by the pixel delta
            // (negative scrolls down, like a trackpad swipe up).
            step if step.starts_with("scroll=") => {
                let mut parts = step["scroll=".len()..].splitn(3, ':');
                let parsed = (|| {
                    let x = parts.next()?.trim().parse::<f32>().ok()?;
                    let y = parts.next()?.trim().parse::<f32>().ok()?;
                    let dy = parts.next()?.trim().parse::<f32>().ok()?;
                    Some((x, y, dy))
                })();
                let Some((x, y, dy)) = parsed else {
                    tracing::error!("bad screenshot step {step}");
                    return;
                };
                let position = gpui::point(gpui::px(x), gpui::px(y));
                window.draw(cx).clear(cx);
                window.dispatch_event(
                    gpui::PlatformInput::MouseMove(gpui::MouseMoveEvent {
                        position,
                        pressed_button: None,
                        modifiers: Default::default(),
                    }),
                    cx,
                );
                window.draw(cx).clear(cx);
                window.dispatch_event(
                    gpui::PlatformInput::ScrollWheel(gpui::ScrollWheelEvent {
                        position,
                        delta: gpui::ScrollDelta::Pixels(gpui::point(gpui::px(0.0), gpui::px(dy))),
                        modifiers: Default::default(),
                        touch_phase: gpui::TouchPhase::Moved,
                    }),
                    cx,
                );
                window.draw(cx).clear(cx);
            }
            // `filedrop=640:380:/tmp/a;/tmp/b` drops files from the OS at that point.
            step if step.starts_with("filedrop=") => {
                let mut parts = step["filedrop=".len()..].splitn(3, ':');
                let (Some(x), Some(y), Some(paths)) = (parts.next(), parts.next(), parts.next())
                else {
                    tracing::error!("bad screenshot step {step}");
                    return;
                };
                let (Ok(x), Ok(y)) = (x.trim().parse::<f32>(), y.trim().parse::<f32>()) else {
                    tracing::error!("bad screenshot step {step}");
                    return;
                };
                let position = gpui::point(gpui::px(x), gpui::px(y));
                let paths = gpui::ExternalPaths(paths.split(';').map(Into::into).collect());
                window.draw(cx).clear(cx);
                for event in [
                    gpui::FileDropEvent::Entered { position, paths },
                    gpui::FileDropEvent::Pending { position },
                    gpui::FileDropEvent::Submit { position },
                ] {
                    window.dispatch_event(gpui::PlatformInput::FileDrop(event), cx);
                    window.draw(cx).clear(cx);
                }
            }
            // `drag=100:200>600:300` presses at the first point and moves to the second with the
            // button held (the shot shows the drag in flight); `drop=` also releases it.
            step if step.starts_with("drag=") || step.starts_with("drop=") => {
                let (kind, span) = step.split_once('=').unwrap_or_default();
                let point = |at: &str| {
                    let (x, y) = at.split_once(':')?;
                    Some(gpui::point(
                        gpui::px(x.trim().parse::<f32>().ok()?),
                        gpui::px(y.trim().parse::<f32>().ok()?),
                    ))
                };
                let Some((from, to)) = span
                    .split_once('>')
                    .and_then(|(a, b)| Some((point(a)?, point(b)?)))
                else {
                    tracing::error!("bad screenshot step {step}");
                    return;
                };
                window.draw(cx).clear(cx);
                window.dispatch_event(
                    gpui::PlatformInput::MouseDown(gpui::MouseDownEvent {
                        button: gpui::MouseButton::Left,
                        position: from,
                        modifiers: Default::default(),
                        click_count: 1,
                        first_mouse: false,
                    }),
                    cx,
                );
                window.draw(cx).clear(cx);
                for i in 1..=8 {
                    let t = i as f32 / 8.0;
                    let position =
                        gpui::point(from.x + (to.x - from.x) * t, from.y + (to.y - from.y) * t);
                    window.dispatch_event(
                        gpui::PlatformInput::MouseMove(gpui::MouseMoveEvent {
                            position,
                            pressed_button: Some(gpui::MouseButton::Left),
                            modifiers: Default::default(),
                        }),
                        cx,
                    );
                    window.draw(cx).clear(cx);
                }
                if kind == "drop" {
                    window.dispatch_event(
                        gpui::PlatformInput::MouseUp(gpui::MouseUpEvent {
                            button: gpui::MouseButton::Left,
                            position: to,
                            modifiers: Default::default(),
                            click_count: 1,
                        }),
                        cx,
                    );
                    window.draw(cx).clear(cx);
                }
            }
            step if step.starts_with("mouse=") || step.starts_with("click=") => {
                let (kind, at) = step.split_once('=').unwrap_or_default();
                let Some((x, y)) = at.split_once(':').and_then(|(x, y)| {
                    Some((x.trim().parse::<f32>().ok()?, y.trim().parse::<f32>().ok()?))
                }) else {
                    tracing::error!("bad screenshot step {step}");
                    return;
                };
                let position = gpui::point(gpui::px(x), gpui::px(y));
                // Hit testing uses the last frame's hitboxes: draw one first.
                window.draw(cx).clear(cx);
                window.dispatch_event(
                    gpui::PlatformInput::MouseMove(gpui::MouseMoveEvent {
                        position,
                        pressed_button: None,
                        modifiers: Default::default(),
                    }),
                    cx,
                );
                if kind == "click" {
                    window.draw(cx).clear(cx);
                    for down in [true, false] {
                        let event = if down {
                            gpui::PlatformInput::MouseDown(gpui::MouseDownEvent {
                                button: gpui::MouseButton::Left,
                                position,
                                modifiers: Default::default(),
                                click_count: 1,
                                first_mouse: false,
                            })
                        } else {
                            gpui::PlatformInput::MouseUp(gpui::MouseUpEvent {
                                button: gpui::MouseButton::Left,
                                position,
                                modifiers: Default::default(),
                                click_count: 1,
                            })
                        };
                        window.dispatch_event(event, cx);
                        // Click handlers listen for the mouse-up only after a frame with the
                        // pending mouse-down.
                        window.draw(cx).clear(cx);
                    }
                }
            }
            step => {
                let action = if let Some(query) = step.strip_prefix("palette=") {
                    let data = serde_json::json!({ "query": query });
                    cx.build_action("palette::Open", Some(data))
                } else if let Some(name) = step.strip_prefix("action=") {
                    cx.build_action(name, None)
                } else {
                    tracing::error!("unknown screenshot step {step}");
                    return;
                };
                match action {
                    Ok(action) => window.dispatch_action(action, cx),
                    Err(err) => tracing::error!("screenshot step {step}: {err}"),
                }
            }
        }
    }

    pub fn schedule(window: AnyWindowHandle, cx: &mut App) {
        let Some(path) = std::env::var_os("KUBYL_SCREENSHOT") else {
            return;
        };
        let actions = std::env::var("KUBYL_SCREENSHOT_ACTIONS").unwrap_or_default();
        cx.spawn(async move |cx| {
            let executor = cx.background_executor().clone();
            let settle = || executor.timer(Duration::from_millis(1500));
            settle().await;
            // Steps run in order. `keys=: c e r t enter` types keystrokes (a frame is drawn
            // before each, so focused inputs have their input handler), `wait=500` pauses.
            for step in actions.split(',').map(str::trim).filter(|s| !s.is_empty()) {
                if let Some(keys) = step.strip_prefix("keys=") {
                    for key in keys.split_whitespace() {
                        let Ok(keystroke) = gpui::Keystroke::parse(key) else {
                            tracing::error!("bad keystroke {key}");
                            continue;
                        };
                        window
                            .update(cx, |_, window, cx| {
                                window.draw(cx).clear(cx);
                                let handled = window.dispatch_keystroke(keystroke, cx);
                                tracing::info!(
                                    key,
                                    handled,
                                    contexts = ?window.context_stack(),
                                    "screenshot keystroke"
                                );
                            })
                            .ok();
                        executor.timer(Duration::from_millis(150)).await;
                    }
                    continue;
                }
                if let Some(ms) = step.strip_prefix("wait=") {
                    let ms = ms.parse().unwrap_or(500);
                    executor.timer(Duration::from_millis(ms)).await;
                    continue;
                }
                window
                    .update(cx, |_, window, cx| run_step(step, window, cx))
                    .ok();
                executor.timer(Duration::from_millis(150)).await;
            }
            settle().await;
            // Draw fresh frames: macOS doesn't drive frames while the display sleeps, is locked
            // or the window is occluded, and render_to_image only rasterizes the last scene. The
            // first draw starts animations (dialogs slide in), the second shows them finished.
            window
                .update(cx, |_, window, cx| window.draw(cx).clear(cx))
                .ok();
            executor.timer(Duration::from_millis(600)).await;
            let image = window.update(cx, |_, window, cx| {
                window.draw(cx).clear(cx);
                window.render_to_image()
            });
            match image {
                Ok(Ok(image)) => match image.save(&path) {
                    Ok(()) => tracing::info!("screenshot saved to {}", path.display()),
                    Err(err) => tracing::error!("failed to save screenshot: {err}"),
                },
                Ok(Err(err)) | Err(err) => tracing::error!("failed to render screenshot: {err:#}"),
            }
            cx.update(|cx| cx.quit());
        })
        .detach();
    }
}
