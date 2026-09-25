//! Service web views (board 10): an embedded browser over temporary port-forwards.
//!
//! Every HTTP(S) port of a Service or Pod (and every Ingress backend) gets a "Web view" button
//! (details, the Services table, `w` in lists, `> Web View: Open…`). It starts a hidden
//! loopback forward ([`forward`]) and opens the page in a tab ([`view`]); closing the last tab
//! stops the forward.
//!
//! - [`native`]: the platform web view (WKWebView, WebView2, WebKitGTK through `wry`).
//! - [`host`]: keeps native views in step with GPUI's frames (placement, hiding).
//! - [`forward`]: reference-counted temporary forwards on `kubyl_portforward`.
//! - [`target`]: service ports, HTTP detection, presets; [`store`]: settings and memory.
//! - [`details`]: the "Web views" details section; [`picker`]: port pickers.

pub mod cert;
pub mod details;
pub mod forward;
pub mod host;
pub mod native;
pub mod picker;
pub mod store;
pub mod target;
pub mod view;

use std::sync::Arc;

use gpui::{
    Action, AnyView, App, AppContext as _, Context, IntoElement, KeyBinding, Render, Window,
    actions, canvas, deferred, div, prelude::*,
};
use kubyl_core::actions::{ActivateDockPanel, OpenView};
use kubyl_core::{
    ActionRegistry, ActionSpec, CellButton, CellValue, ChromeRegistry, ColumnDef, ColumnProvider,
    ColumnWidth, Notification, NotificationCenter, ResourceColumns, ResourceRef, StatusBarItem,
    StatusBarPosition, ViewKind, ViewRegistry, ViewRequest,
};
use kubyl_resources::ResourceSelection;
use kubyl_resources::store::{ResourceStores, StoreKey, object_key};
use kubyl_ui::{ActiveColors, Icon, IconName, h_flex, u};
use serde::Deserialize;
use serde_json::Value;

pub use host::snapshots;

use forward::WebForwards;
use target::{
    BackendPort, Scheme, TargetKind, WebPort, WebTarget, ingress_backends, ports_of, preset_for,
    preset_names,
};
use view::{OpenRequest, PendingOpens, WebViewTab};

/// The view kind of web-view tabs (never restored: forwards don't start by themselves).
pub const VIEW_KIND: &str = "web_view";

actions!(
    webview,
    [
        /// Opens a web view for the selected Service, Pod or Ingress (a port picker when it
        /// has several web ports).
        OpenSelected,
        /// Picks a web port of the Services in the active namespace.
        OpenPicker,
        ReloadPage,
        OpenInBrowser,
        CopyUrl,
        ClearSiteData,
        DevTools,
        FocusAddress,
        ZoomPageIn,
        ZoomPageOut,
        ResetPageZoom,
        /// Stops the forward of the focused web view and closes its tabs.
        StopForward,
        Cut,
        Copy,
        Paste,
        SelectAll,
        Undo,
        Redo,
    ]
);

#[cfg(debug_assertions)]
actions!(
    webview,
    [
        /// Debug builds: posts `$KUBYL_WEBVIEW_TEST_INPUT` to the focused web view as real OS
        /// events (see `native::macos::post_test_input`), to check key routing end to end.
        DebugInput,
    ]
);

/// Opens a web view for `port` of a Service or Pod (buttons in details and tables).
#[derive(Clone, PartialEq, Debug, Deserialize, Action)]
#[action(namespace = webview, no_json)]
pub struct OpenWebView {
    pub target: ResourceRef,
    pub port: u16,
    /// Where to start instead of the remembered page (an Ingress path).
    pub path: Option<String>,
    /// Ask for the scheme and start path first ("Open as web view…" on non-HTTP ports).
    pub ask: bool,
}

const CONTEXT: &str = "WebView";

/// Registers the view, actions, details section, column and status bar item.
pub fn init(cx: &mut App) {
    store::init(cx);
    picker::init(cx);
    WebForwards::install(cx);
    cx.default_global::<PendingOpens>();

    ViewRegistry::register(cx, ViewKind::Custom(VIEW_KIND.into()), |_, window, cx| {
        let request = cx.global_mut::<PendingOpens>().0.pop_front()?;
        Some(kubyl_core::new_tab(cx, |cx| {
            WebViewTab::new(request, window, cx)
        }))
    });
    ChromeRegistry::add_status_item(cx, WebStatusItem);
    // Downloads nobody saved (the app quit while asking) don't pile up.
    let staging = view::data_dir().join("downloads");
    cx.background_executor()
        .spawn(async move { remove_stale_downloads(&staging) })
        .detach();
    ChromeRegistry::add_details_section(cx, details::WebViewDetails);
    ResourceColumns::extend(cx, "", "Service", WebColumn);

    // Core Services and Pods only (a Knative Service is `services` too), like the details.
    let applies = |target: &ResourceRef, _: &kubyl_core::ClusterCaps| {
        (target.gvr.group.is_empty() && matches!(target.gvr.resource.as_str(), "services" | "pods"))
            || (target.gvr.resource == "ingresses" && target.gvr.group == "networking.k8s.io")
    };
    // Web views are a read path: allowed on read-only clusters too (the toolbar says so).
    ActionRegistry::register(
        cx,
        ActionSpec::new("Resource: Open Web View", OpenSelected)
            .hint("Web")
            .bind("w", Some("ResourceList"))
            .available_when(applies),
    );
    ActionRegistry::register(cx, ActionSpec::new("Web View: Open…", OpenPicker));
    register_in_view(cx, "Web View: Reload", ReloadPage, Some("f5"));
    register_in_view(cx, "Web View: Open in Browser", OpenInBrowser, None);
    register_in_view(cx, "Web View: Copy URL", CopyUrl, None);
    register_in_view(cx, "Web View: Clear Site Data", ClearSiteData, None);
    register_in_view(
        cx,
        "Web View: Developer Tools",
        DevTools,
        Some("secondary-alt-i"),
    );
    register_in_view(
        cx,
        "Web View: Focus Address Bar",
        FocusAddress,
        Some("secondary-l"),
    );
    register_in_view(cx, "Web View: Zoom In", ZoomPageIn, Some("secondary-="));
    register_in_view(cx, "Web View: Zoom Out", ZoomPageOut, Some("secondary--"));
    register_in_view(
        cx,
        "Web View: Reset Zoom",
        ResetPageZoom,
        Some("secondary-0"),
    );
    register_in_view(cx, "Web View: Stop and Close Tabs", StopForward, None);
    cx.bind_keys([KeyBinding::new("secondary-+", ZoomPageIn, Some(CONTEXT))]);
    // WKWebView only gets editing commands through the responder chain (an Edit menu),
    // which Kubyl doesn't have. WebView2 and WebKitGTK handle these keys themselves.
    if cfg!(target_os = "macos") {
        cx.bind_keys([
            KeyBinding::new("cmd-c", Copy, Some(CONTEXT)),
            KeyBinding::new("cmd-x", Cut, Some(CONTEXT)),
            KeyBinding::new("cmd-v", Paste, Some(CONTEXT)),
            KeyBinding::new("cmd-a", SelectAll, Some(CONTEXT)),
            KeyBinding::new("cmd-z", Undo, Some(CONTEXT)),
            KeyBinding::new("cmd-shift-z", Redo, Some(CONTEXT)),
        ]);
    }

    cx.on_action(|action: &OpenWebView, cx| {
        let action = action.clone();
        with_window(cx, move |window, cx| open_action(&action, window, cx));
    });
    cx.on_action(|_: &OpenSelected, cx| {
        with_window(cx, open_selected);
    });
    cx.on_action(|_: &OpenPicker, cx| {
        with_window(cx, picker::open_namespace_picker);
    });
}

/// Removes staged downloads older than a day.
fn remove_stale_downloads(staging: &std::path::Path) {
    let Ok(entries) = std::fs::read_dir(staging) else {
        return;
    };
    let day = std::time::Duration::from_secs(24 * 60 * 60);
    for entry in entries.flatten() {
        let old = entry
            .metadata()
            .and_then(|m| m.modified())
            .ok()
            .and_then(|modified| modified.elapsed().ok())
            .is_some_and(|age| age > day);
        if old {
            std::fs::remove_dir_all(entry.path()).ok();
        }
    }
}

/// Registers an action of the focused web view (palette, and its default key binding).
fn register_in_view(cx: &mut App, name: &'static str, action: impl Action, keys: Option<&str>) {
    let spec = ActionSpec::new(name, action);
    let spec = match keys {
        Some(keys) => spec.bind(keys, Some(CONTEXT)),
        None => spec,
    };
    ActionRegistry::register(cx, spec);
}

/// The platform pump runs (see [`native::pump`]).
struct Pumping;

impl gpui::Global for Pumping {}

/// Keeps pumping the platform's web view work while any web view exists (Linux).
pub(crate) fn start_pump(cx: &mut App) {
    if !native::NEEDS_PUMP || cx.has_global::<Pumping>() {
        return;
    }
    cx.set_global(Pumping);
    cx.spawn(async move |cx| {
        loop {
            native::pump();
            if !host::any_views() {
                break;
            }
            cx.background_executor()
                .timer(std::time::Duration::from_millis(8))
                .await;
        }
        // Let the last views' teardown finish, then stop until a view is created again.
        native::pump();
        cx.update(|cx| cx.remove_global::<Pumping>());
    })
    .detach();
}

/// Runs `f` in the active window, after the dispatching window's update.
pub(crate) fn with_window(cx: &mut App, f: impl FnOnce(&mut Window, &mut App) + 'static) {
    cx.defer(move |cx| {
        let window = cx.active_window().or_else(|| cx.windows().first().copied());
        if let Some(window) = window {
            window.update(cx, |_, window, cx| f(window, cx)).ok();
        }
    });
}

/// Opens a web-view tab in the active pane of `window`.
pub(crate) fn open_in(request: OpenRequest, window: &mut Window, cx: &mut App) {
    if let Err(message) = native::supported()
        && store::WebViewSettings::get(cx).open_in != store::OpenIn::Browser
    {
        tracing::info!("web view in the browser: {message}");
    }
    cx.default_global::<PendingOpens>().0.push_back(request);
    window.dispatch_action(
        Box::new(OpenView(ViewRequest::new(ViewKind::Custom(
            VIEW_KIND.into(),
        )))),
        cx,
    );
}

/// The object `target` from the watch caches, if a list or the details have it.
pub(crate) fn cached_object(target: &ResourceRef, cx: &App) -> Option<Arc<Value>> {
    let name = target.name.as_deref()?;
    let key = object_key(target.namespace.as_deref(), name);
    [
        StoreKey::new(
            target.cluster.clone(),
            target.gvr.clone(),
            target.namespace.clone(),
        ),
        StoreKey::new(target.cluster.clone(), target.gvr.clone(), None),
    ]
    .iter()
    .filter_map(|store| ResourceStores::peek(cx, store))
    .find_map(|store| store.read(cx).get(&key).cloned())
}

/// The request for `port` of a Service or Pod, with the scheme and preset its object suggests.
pub(crate) fn request_for(
    target: &ResourceRef,
    port: u16,
    object: Option<&Value>,
) -> Option<OpenRequest> {
    let web_target = WebTarget::new(target, port)?;
    let mut request = OpenRequest::new(web_target.clone());
    let web_port = object.and_then(|o| {
        ports_of(web_target.kind, o)
            .into_iter()
            .find(|p| p.port == port)
    });
    request.detected = match &web_port {
        Some(p) => p.scheme,
        None => {
            let (_, https) = kubyl_portforward::resolve::http_kind(port, None, None);
            if https { Scheme::Https } else { Scheme::Http }
        }
    };
    if let Some(object) = object {
        let names = preset_names(object);
        let names: Vec<&str> = names.iter().map(String::as_str).collect();
        if let Some(preset) = preset_for(&names, port) {
            request.preset_path = Some(preset.start_path);
            if let Some(scheme) = preset.scheme {
                request.detected = scheme;
            }
        }
    }
    Some(request)
}

fn open_action(action: &OpenWebView, window: &mut Window, cx: &mut App) {
    let object = cached_object(&action.target, cx);
    let Some(mut request) = request_for(&action.target, action.port, object.as_deref()) else {
        NotificationCenter::push(
            cx,
            Notification::error("Web views open for ports of Services and Pods."),
        );
        return;
    };
    request.path = action.path.clone();
    if action.ask {
        picker::open_as(request, window, cx);
    } else {
        open_in(request, window, cx);
    }
}

/// `w` in a list: the selected object's only web port, else a picker of its ports.
fn open_selected(window: &mut Window, cx: &mut App) {
    let Some(selected) = ResourceSelection::global(cx).primary().cloned() else {
        return;
    };
    let target = selected.target.clone();
    let object = selected
        .object
        .clone()
        .or_else(|| cached_object(&target, cx));
    let Some(object) = object else {
        NotificationCenter::push(cx, Notification::info("The object isn't loaded yet."));
        return;
    };
    let rows = picker::rows_for_object(&target, &object, cx);
    let web: Vec<_> = rows.iter().filter(|r| r.web).collect();
    match web.as_slice() {
        [only] => open_in(only.request.clone(), window, cx),
        [] if rows.is_empty() => NotificationCenter::push(
            cx,
            Notification::info(format!(
                "{} has no TCP ports to open.",
                target.name.clone().unwrap_or_default()
            )),
        ),
        _ => picker::open_rows(
            format!(
                "Open a web view · {}",
                target.name.clone().unwrap_or_default()
            ),
            rows,
            window,
            cx,
        ),
    }
}

/// Resolves an Ingress backend's port against its Service (a named port needs the Service).
pub(crate) fn backend_port(port: &BackendPort, service: Option<&Value>) -> Option<u16> {
    match port {
        BackendPort::Number(n) => Some(*n),
        BackendPort::Name(name) => service.and_then(|service| {
            ports_of(TargetKind::Service, service)
                .into_iter()
                .find(|p| p.name.as_deref() == Some(name.as_str()))
                .map(|p| p.port)
        }),
    }
}

/// The web ports of an object, for buttons (`Service`, `Pod`) or the Ingress backends.
pub(crate) fn web_ports(kind: TargetKind, object: &Value) -> Vec<WebPort> {
    ports_of(kind, object)
        .into_iter()
        .filter(|p| p.web)
        .collect()
}

/// Ingress backends (for the picker and details).
pub(crate) fn backends(object: &Value) -> Vec<target::IngressBackend> {
    ingress_backends(object)
}

/// The "Web" column of the Services table: one button per HTTP port.
struct WebColumn;

impl ColumnProvider for WebColumn {
    fn columns(&self) -> Vec<ColumnDef> {
        vec![ColumnDef::new("web", "Web", ColumnWidth::Fixed(150.0))]
    }

    fn cell(&self, object: &Value, _: &str) -> CellValue {
        let buttons: Vec<CellButton> = web_ports(TargetKind::Service, object)
            .into_iter()
            .take(3)
            .map(|port| {
                let number = port.port;
                CellButton::new(port.port.to_string(), move |target: &ResourceRef| {
                    Box::new(OpenWebView {
                        target: target.clone(),
                        port: number,
                        path: None,
                        ask: false,
                    })
                })
                .icon(IconName::Globe.path())
                .tooltip(format!("Open port {} in a web view", port.port))
            })
            .collect();
        if buttons.is_empty() {
            CellValue::Empty
        } else {
            CellValue::Buttons(buttons)
        }
    }
}

struct WebStatusItem;

impl StatusBarItem for WebStatusItem {
    fn id(&self) -> &'static str {
        "webview"
    }

    fn position(&self) -> StatusBarPosition {
        StatusBarPosition::Right
    }

    fn order(&self) -> i32 {
        -10
    }

    fn build(&self, _: &mut Window, cx: &mut App) -> AnyView {
        cx.new(|cx| {
            if let Some(forwards) = WebForwards::try_global(cx) {
                cx.observe(&forwards, |_, _, cx| cx.notify()).detach();
            }
            WebStatus
        })
        .into()
    }
}

/// `2 web forwards` in the status bar, plus the per-frame hook that shows and hides the
/// native views of the window ([`host::end_frame`]).
struct WebStatus;

impl Render for WebStatus {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let colors = cx.colors().clone();
        let count = WebForwards::try_global(cx)
            .map(|f| f.read(cx).running().count())
            .unwrap_or_default();
        h_flex()
            .id("web-forwards")
            .gap(u(5.0))
            .when(count > 0, |this| {
                this.cursor_pointer()
                    .text_color(colors.text)
                    .child(Icon::new(IconName::Globe).size(12.0).color(colors.accent))
                    .child(match count {
                        1 => "1 web forward".to_string(),
                        n => format!("{n} web forwards"),
                    })
                    .on_click(|_, window, cx| {
                        window.dispatch_action(
                            Box::new(ActivateDockPanel(kubyl_logs::dock::PANEL_ID.to_string())),
                            cx,
                        )
                    })
            })
            .child(
                deferred(canvas(
                    |_, _, _| (),
                    |_, _, window, cx| host::end_frame(window, cx),
                ))
                .priority(usize::MAX),
            )
            .child(div())
    }
}
