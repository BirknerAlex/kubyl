//! The platform web view behind a web-view tab: WKWebView (macOS), WebView2 (Windows) or
//! WebKitGTK (Linux), created through `wry` as a child view of the GPUI window.
//!
//! Everything here runs on the UI thread. Callbacks from the platform view never touch GPUI
//! state directly: they send [`NativeEvent`]s into a channel the tab reads.
//!
//! Nothing Kubyl knows (tokens, client certificates, kubeconfig data) is ever passed to the
//! web view: it only gets a loopback URL, a data store and its own settings.

use std::cell::RefCell;
use std::path::{Path, PathBuf};
use std::rc::Rc;

use futures::channel::mpsc::UnboundedSender;
use gpui::{Bounds, Keystroke, Pixels};
use raw_window_handle::{HandleError, HasWindowHandle, RawWindowHandle, WindowHandle};

#[cfg(target_os = "linux")]
mod linux;
#[cfg(target_os = "macos")]
mod macos;
#[cfg(target_os = "windows")]
mod windows;

/// Code for platforms with a `wry` backend in this build.
macro_rules! with_wry {
    ($($item:tt)*) => {
        #[cfg(any(target_os = "macos", target_os = "windows", target_os = "linux"))]
        $($item)*
    };
}

/// What the platform view reports to its tab.
#[derive(Clone, Debug, PartialEq)]
pub enum NativeEvent {
    TitleChanged(String),
    /// The main frame started loading `url`.
    LoadStarted(String),
    /// The main frame finished loading `url`.
    LoadFinished(String),
    /// The page asked for a new window (`target=_blank`, `window.open`); it wasn't opened.
    NewWindow(String),
    /// A download started into Kubyl's staging directory.
    DownloadStarted {
        url: String,
        staged: PathBuf,
    },
    /// A download ended (`staged` is where it was written).
    DownloadFinished {
        url: String,
        staged: Option<PathBuf>,
        ok: bool,
    },
    /// The server's certificate isn't trusted and wasn't accepted for this service (leaf
    /// first, DER).
    CertificateRejected {
        chain: Vec<Vec<u8>>,
    },
    /// The web content got the OS keyboard focus (a click into the page).
    Focused,
    /// A Kubyl shortcut pressed while the web content had the keyboard focus.
    Shortcut(Keystroke),
    /// The page's own window was closed (the separate-window fallback).
    WindowClosed,
    /// The web content process crashed or was killed.
    Crashed,
}

/// How the view stores cookies, local storage and caches.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Storage {
    /// Persistent, private to one (cluster, namespace, service): `id` names it.
    Isolated { id: [u8; 16] },
    /// Nothing is written to disk; gone when the view closes.
    Private,
}

/// Where the page is shown.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Placement {
    /// A child view inside the tab (macOS, Windows, Linux on X11).
    Embedded,
    /// Its own window (Linux on Wayland, which can't embed foreign surfaces).
    Window,
}

#[derive(Clone, Debug)]
pub struct NativeOptions {
    pub url: String,
    /// Title of the separate window ([`Placement::Window`]).
    pub title: String,
    pub storage: Storage,
    /// Kubyl's web-view data directory (WebView2 user data, WebKitGTK data per service).
    pub data_dir: PathBuf,
    /// Where downloads are written until the user picks a place.
    pub staging_dir: PathBuf,
    pub devtools: bool,
    /// Page zoom (1.0 = 100%).
    pub zoom: f64,
    /// Leaf certificate fingerprints (SHA-256) the user accepted for this service.
    pub accepted_certs: Rc<RefCell<Vec<[u8; 32]>>>,
    /// Keystrokes that belong to Kubyl while the page has focus (⌘K, ⌘W…), on platforms where
    /// the page would otherwise swallow them (Windows, Linux).
    pub shortcuts: Rc<RefCell<Vec<Keystroke>>>,
    /// Session cookies set before the first load ([`crate::session`]).
    pub cookies: Vec<crate::session::SessionCookie>,
}

/// The GPUI window's native handle, for `wry` (which needs [`HasWindowHandle`]).
///
/// Captured inside a window update, used outside of it: creating a WebView2 controller pumps
/// the Win32 message loop, which must not happen while GPUI's `App` is borrowed.
#[derive(Clone, Copy)]
pub struct ParentWindow(RawWindowHandle);

impl ParentWindow {
    pub fn of(window: &gpui::Window) -> anyhow::Result<Self> {
        let raw = HasWindowHandle::window_handle(window)?.as_raw();
        // GPUI's X11 backend hands out XCB handles; wry wants Xlib. Both name the same X
        // window.
        #[cfg(target_os = "linux")]
        let raw = match raw {
            RawWindowHandle::Xcb(xcb) => RawWindowHandle::Xlib(
                raw_window_handle::XlibWindowHandle::new(xcb.window.get() as _),
            ),
            other => other,
        };
        Ok(Self(raw))
    }

    /// Where views can go in this window.
    pub fn placement(&self) -> Placement {
        match self.0 {
            RawWindowHandle::Wayland(_) => Placement::Window,
            _ => Placement::Embedded,
        }
    }
}

impl HasWindowHandle for ParentWindow {
    fn window_handle(&self) -> Result<WindowHandle<'_>, HandleError> {
        // SAFETY: the handle comes from a live GPUI window; views are dropped with their tab,
        // before the window goes away.
        Ok(unsafe { WindowHandle::borrow_raw(self.0) })
    }
}

/// Runs the platform's pending web view work: WebKitGTK lives in GTK's main loop, which
/// Kubyl's event loop doesn't run. A no-op elsewhere.
pub fn pump() {
    #[cfg(target_os = "linux")]
    linux::pump();
}

/// Whether the platform needs [`pump`] called regularly while web views exist.
pub const NEEDS_PUMP: bool = cfg!(target_os = "linux");

/// Whether web views can be shown in this build.
pub fn supported() -> Result<(), String> {
    #[cfg(any(target_os = "macos", target_os = "windows", target_os = "linux"))]
    {
        Ok(())
    }
    #[cfg(not(any(target_os = "macos", target_os = "windows", target_os = "linux")))]
    {
        Err("Web views aren't available on this platform.".into())
    }
}

/// A platform web view embedded in a GPUI window (or in its own window on Wayland).
pub struct NativeWebView {
    #[cfg(any(target_os = "macos", target_os = "windows", target_os = "linux"))]
    webview: wry::WebView,
    placement: Placement,
    #[cfg(target_os = "macos")]
    _platform: macos::Attached,
    #[cfg(target_os = "windows")]
    _platform: windows::Attached,
    #[cfg(target_os = "linux")]
    platform: linux::Attached,
    /// Keeps the service's WebKitGTK context alive while its views exist.
    #[cfg(target_os = "linux")]
    _context: Option<Rc<RefCell<wry::WebContext>>>,
    #[cfg(target_os = "linux")]
    bounds: std::cell::Cell<Option<Bounds<Pixels>>>,
}

impl NativeWebView {
    /// Creates the view hidden; [`Self::set_bounds`] and [`Self::set_visible`] place it.
    #[allow(unused_variables)]
    pub fn create(
        parent: &ParentWindow,
        options: NativeOptions,
        events: UnboundedSender<NativeEvent>,
    ) -> anyhow::Result<Self> {
        #[cfg(not(any(target_os = "macos", target_os = "windows", target_os = "linux")))]
        {
            Err(anyhow::anyhow!(supported().unwrap_err()))
        }
        #[cfg(any(target_os = "macos", target_os = "windows", target_os = "linux"))]
        {
            use std::collections::HashMap;

            use wry::{NewWindowResponse, PageLoadEvent, Rect, WebViewBuilder};

            // WebKitGTK contexts need GTK: started with the first web view, not with Kubyl.
            #[cfg(target_os = "linux")]
            linux::ensure_gtk()?;
            let placement = parent.placement();
            let downloads = Rc::new(RefCell::new(HashMap::<String, Vec<PathBuf>>::new()));
            let staging_dir = options.staging_dir.clone();
            // WebView2: one user data folder, a profile per service (`windows::configure`).
            #[cfg(target_os = "windows")]
            let mut context = wry::WebContext::new(Some(options.data_dir.join("webview2")));
            #[cfg(target_os = "windows")]
            let builder = WebViewBuilder::new_with_web_context(&mut context);
            #[cfg(target_os = "linux")]
            let shared_context = linux_context(&options);
            #[cfg(target_os = "linux")]
            let mut context = shared_context.as_ref().map(|c| c.borrow_mut());
            #[cfg(target_os = "linux")]
            let builder = match context.as_deref_mut() {
                Some(context) => WebViewBuilder::new_with_web_context(context),
                None => WebViewBuilder::new(),
            };
            #[cfg(target_os = "macos")]
            let builder = WebViewBuilder::new();
            // With session cookies, the page loads once they're set (below).
            let builder = if options.cookies.is_empty() {
                builder.with_url(options.url.clone())
            } else {
                builder
            };
            let builder = builder
                .with_visible(placement == Placement::Window)
                .with_bounds(Rect {
                    position: wry::dpi::LogicalPosition::new(0.0, 0.0).into(),
                    size: wry::dpi::LogicalSize::new(1.0, 1.0).into(),
                })
                .with_devtools(options.devtools)
                .with_clipboard(true)
                .with_hotkeys_zoom(false)
                .with_accept_first_mouse(true)
                .with_back_forward_navigation_gestures(true)
                .with_incognito(options.storage == Storage::Private)
                .with_document_title_changed_handler({
                    let events = events.clone();
                    move |title| {
                        events.unbounded_send(NativeEvent::TitleChanged(title)).ok();
                    }
                })
                .with_on_page_load_handler({
                    let events = events.clone();
                    move |event, url| {
                        let event = match event {
                            PageLoadEvent::Started => NativeEvent::LoadStarted(url),
                            PageLoadEvent::Finished => NativeEvent::LoadFinished(url),
                        };
                        events.unbounded_send(event).ok();
                    }
                })
                .with_new_window_req_handler({
                    let events = events.clone();
                    move |url, _| {
                        events.unbounded_send(NativeEvent::NewWindow(url)).ok();
                        NewWindowResponse::Deny
                    }
                })
                .with_download_started_handler({
                    let events = events.clone();
                    let downloads = downloads.clone();
                    move |url, path| {
                        let Some(staged) = staged_path(&staging_dir, path) else {
                            return false;
                        };
                        *path = staged.clone();
                        downloads
                            .borrow_mut()
                            .entry(url.clone())
                            .or_default()
                            .push(staged.clone());
                        events
                            .unbounded_send(NativeEvent::DownloadStarted { url, staged })
                            .ok();
                        true
                    }
                })
                .with_download_completed_handler({
                    let events = events.clone();
                    move |url, path, ok| {
                        // macOS doesn't report the path: take the oldest staged one of the URL.
                        let staged = take_staged(&mut downloads.borrow_mut(), &url, path);
                        events
                            .unbounded_send(NativeEvent::DownloadFinished { url, staged, ok })
                            .ok();
                    }
                });

            #[cfg(target_os = "macos")]
            let builder = macos::configure(builder, &options);
            #[cfg(target_os = "windows")]
            let builder = windows::configure(builder, &options);

            #[cfg(target_os = "linux")]
            let (webview, platform) = linux::build(builder, parent, &options, &events)?;
            #[cfg(not(target_os = "linux"))]
            let webview = builder.build_as_child(parent)?;
            #[cfg(any(target_os = "windows", target_os = "linux"))]
            drop(context);

            if (options.zoom - 1.0).abs() > f64::EPSILON {
                webview.zoom(options.zoom).ok();
            }
            #[cfg(target_os = "macos")]
            let platform = macos::attach(&webview, &options, events)?;
            #[cfg(target_os = "windows")]
            let platform = windows::attach(&webview, &options, events)?;
            #[cfg(target_os = "linux")]
            let platform = linux::attach(&webview, platform, &options, events)?;
            if !options.cookies.is_empty() {
                // After `attach`: the certificate hooks must be in place for the first load.
                let cookies = crate::session::wry_cookies(&options.url, &options.cookies);
                #[cfg(target_os = "macos")]
                macos::set_cookies_then_load(&webview, &cookies, &options.url);
                #[cfg(not(target_os = "macos"))]
                {
                    for cookie in cookies {
                        if let Err(err) = webview.set_cookie(&cookie) {
                            tracing::warn!(
                                name = cookie.name(),
                                "couldn't set a session cookie: {err}"
                            );
                        }
                    }
                    webview.load_url(&options.url).ok();
                }
            }
            Ok(Self {
                webview,
                placement,
                #[cfg(target_os = "linux")]
                _context: shared_context,
                #[cfg(target_os = "linux")]
                bounds: std::cell::Cell::new(None),
                #[cfg(not(target_os = "linux"))]
                _platform: platform,
                #[cfg(target_os = "linux")]
                platform,
            })
        }
    }

    pub fn placement(&self) -> Placement {
        self.placement
    }

    /// Places the view at `bounds` (logical pixels, relative to the window's content area).
    #[allow(unused_variables)]
    pub fn set_bounds(&self, bounds: Bounds<Pixels>) {
        if self.placement == Placement::Window {
            return;
        }
        // wry (macOS) unwraps the view's window.
        #[cfg(target_os = "macos")]
        if !macos::in_window(&self.webview) {
            return;
        }
        #[cfg(target_os = "linux")]
        self.bounds.set(Some(bounds));
        with_wry! {{
            let rect = wry::Rect {
                position: wry::dpi::LogicalPosition::new(
                    f64::from(f32::from(bounds.origin.x)),
                    f64::from(f32::from(bounds.origin.y)),
                )
                .into(),
                size: wry::dpi::LogicalSize::new(
                    f64::from(f32::from(bounds.size.width)).max(1.0),
                    f64::from(f32::from(bounds.size.height)).max(1.0),
                )
                .into(),
            };
            if let Err(err) = self.webview.set_bounds(rect) {
                tracing::debug!("web view bounds: {err}");
            }
        }}
    }

    #[allow(unused_variables)]
    pub fn set_visible(&self, visible: bool) {
        if self.placement == Placement::Window {
            return;
        }
        with_wry! {{
            if !visible {
                self.webview.focus_parent().ok();
            }
            self.webview.set_visible(visible).ok();
            // Showing a WebKitGTK child re-runs GTK's size negotiation, which a foreign X11
            // window never finishes (no frame clock): allocate the bounds again.
            #[cfg(target_os = "linux")]
            if visible && let Some(bounds) = self.bounds.get() {
                self.set_bounds(bounds);
            }
        }}
    }

    /// Gives the page the OS keyboard focus (or raises its window).
    pub fn focus(&self) {
        #[cfg(target_os = "linux")]
        if self.placement == Placement::Window {
            self.platform.present();
            return;
        }
        #[cfg(target_os = "macos")]
        if !macos::in_window(&self.webview) {
            return;
        }
        with_wry! { self.webview.focus().ok(); }
    }

    /// Hands the OS keyboard focus back to the GPUI window.
    pub fn blur(&self) {
        if self.placement == Placement::Window {
            return;
        }
        with_wry! { self.webview.focus_parent().ok(); }
    }

    #[allow(unused_variables)]
    pub fn load_url(&self, url: &str) {
        // Normalized first: wry (macOS) unwraps `NSURL::URLWithString`, which rejects some
        // strings a user can type into the address bar.
        let Ok(url) = url::Url::parse(url) else {
            tracing::debug!(url, "not loading an invalid URL");
            return;
        };
        with_wry! { self.webview.load_url(url.as_str()).ok(); }
    }

    pub fn reload(&self) {
        with_wry! { self.webview.reload().ok(); }
    }

    pub fn back(&self) {
        with_wry! { self.webview.go_back().ok(); }
    }

    pub fn forward(&self) {
        with_wry! { self.webview.go_forward().ok(); }
    }

    pub fn can_go_back(&self) -> bool {
        with_wry! { return self.webview.can_go_back().unwrap_or(false); }
        #[allow(unreachable_code)]
        false
    }

    pub fn can_go_forward(&self) -> bool {
        with_wry! { return self.webview.can_go_forward().unwrap_or(false); }
        #[allow(unreachable_code)]
        false
    }

    /// The main frame's URL.
    /// The page's URL; `None` before a page commits.
    pub fn url(&self) -> Option<String> {
        #[cfg(target_os = "macos")]
        return macos::url(&self.webview);
        #[cfg(any(target_os = "windows", target_os = "linux"))]
        return self.webview.url().ok().filter(|url| !url.is_empty());
        #[allow(unreachable_code)]
        None
    }

    #[allow(unused_variables)]
    pub fn set_zoom(&self, zoom: f64) {
        with_wry! { self.webview.zoom(zoom).ok(); }
    }

    pub fn open_devtools(&self) {
        with_wry! { self.webview.open_devtools(); }
    }

    /// Clears cookies, storage and caches of this view's data store, and (Windows) the
    /// certificates allowed in this session. WebKitGTK keeps allowed certificates in the
    /// service's context until its last view closes.
    pub fn clear_browsing_data(&self) {
        with_wry! { self.webview.clear_all_browsing_data().ok(); }
        #[cfg(target_os = "windows")]
        windows::clear_certificate_decisions(&self.webview);
    }

    /// Sets the title of the page's own window ([`Placement::Window`]).
    #[allow(unused_variables)]
    pub fn set_window_title(&self, title: &str) {
        #[cfg(target_os = "linux")]
        self.platform.set_title(title);
    }

    /// Runs an editing command (`copy`, `paste`…) in the page, for platforms where the page
    /// doesn't get these shortcuts on its own (macOS: Kubyl has no Edit menu).
    #[allow(unused_variables)]
    pub fn edit(&self, command: EditCommand) {
        #[cfg(target_os = "macos")]
        macos::edit(&self.webview, command);
    }

    /// Takes a PNG snapshot of the page as shown; `None` where unsupported or on failure.
    pub fn snapshot(&self, done: Box<dyn FnOnce(Option<Vec<u8>>)>) {
        #[cfg(target_os = "macos")]
        macos::snapshot(&self.webview, done);
        #[cfg(not(target_os = "macos"))]
        done(None);
    }

    /// Test driver (debug builds): real OS input events, see `macos::post_test_input`.
    #[cfg(debug_assertions)]
    #[allow(unused_variables)]
    pub fn post_test_input(&self, script: &str) {
        #[cfg(target_os = "macos")]
        macos::post_test_input(&self.webview, script);
    }
}

/// The WebKitGTK context of a data store, shared by its views: two contexts on one data
/// directory don't see each other's cookies until they're written.
#[cfg(target_os = "linux")]
fn linux_context(options: &NativeOptions) -> Option<Rc<RefCell<wry::WebContext>>> {
    use std::collections::HashMap;
    use std::rc::Weak;

    thread_local! {
        static CONTEXTS: RefCell<HashMap<[u8; 16], Weak<RefCell<wry::WebContext>>>> =
            RefCell::new(HashMap::new());
    }
    let Storage::Isolated { id } = &options.storage else {
        return None;
    };
    CONTEXTS.with_borrow_mut(|contexts| {
        contexts.retain(|_, context| context.strong_count() > 0);
        if let Some(context) = contexts.get(id).and_then(Weak::upgrade) {
            return Some(context);
        }
        let context = Rc::new(RefCell::new(wry::WebContext::new(Some(
            options.data_dir.join(storage_name(id)),
        ))));
        contexts.insert(*id, Rc::downgrade(&context));
        Some(context)
    })
}

/// Editing commands Kubyl routes to the page.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum EditCommand {
    Cut,
    Copy,
    Paste,
    SelectAll,
    Undo,
    Redo,
}

/// A data store's directory / profile name: its id in hex.
pub fn storage_name(id: &[u8; 16]) -> String {
    id.iter().map(|b| format!("{b:02x}")).collect()
}

/// Whether `host` is this machine (the only hosts whose certificates Kubyl ever accepts).
pub fn is_loopback(host: &str) -> bool {
    host.eq_ignore_ascii_case("localhost")
        || host
            .trim_matches(|c| c == '[' || c == ']')
            .parse::<std::net::IpAddr>()
            .is_ok_and(|ip| ip.is_loopback())
}

/// Whether `url` points at this machine.
pub fn is_loopback_url(url: &str) -> bool {
    url::Url::parse(url)
        .ok()
        .and_then(|u| u.host_str().map(is_loopback))
        .unwrap_or(false)
}

/// The DER bytes of the first certificate in a PEM text.
pub fn pem_to_der(pem: &str) -> Option<Vec<u8>> {
    use base64::Engine as _;
    const BEGIN: &str = "-----BEGIN CERTIFICATE-----";
    let start = pem.find(BEGIN)? + BEGIN.len();
    let end = start + pem[start..].find("-----END CERTIFICATE-----")?;
    let body: String = pem[start..end]
        .chars()
        .filter(|c| !c.is_whitespace())
        .collect();
    base64::engine::general_purpose::STANDARD.decode(body).ok()
}

/// Whether a key pressed in the page is one of Kubyl's shortcuts.
pub fn is_shortcut(shortcuts: &[Keystroke], pressed: &Keystroke) -> bool {
    shortcuts
        .iter()
        .any(|s| s.modifiers == pressed.modifiers && s.key.eq_ignore_ascii_case(&pressed.key))
}

/// A unique path in `staging_dir` for a download the platform wanted to write to `default`.
fn staged_path(staging_dir: &Path, default: &Path) -> Option<PathBuf> {
    let name = default
        .file_name()
        .map(|n| n.to_string_lossy().into_owned())
        .filter(|n| !n.is_empty())
        .unwrap_or_else(|| "download".into());
    std::fs::create_dir_all(staging_dir).ok()?;
    let stamp = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_millis())
        .unwrap_or_default();
    (0..1000u32).find_map(|attempt| {
        let dir = staging_dir.join(format!("{stamp}-{attempt}"));
        std::fs::create_dir(&dir).ok().map(|()| dir.join(&name))
    })
}

/// The staged path of a finished download: the reported one, else the oldest staged for `url`.
fn take_staged(
    downloads: &mut std::collections::HashMap<String, Vec<PathBuf>>,
    url: &str,
    reported: Option<PathBuf>,
) -> Option<PathBuf> {
    let queue = downloads.entry(url.to_string()).or_default();
    let staged = match reported {
        Some(path) => {
            queue.retain(|p| *p != path);
            Some(path)
        }
        None if !queue.is_empty() => Some(queue.remove(0)),
        None => None,
    };
    if queue.is_empty() {
        downloads.remove(url);
    }
    staged
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn staged_downloads_get_their_own_directory() {
        let dir = tempfile::tempdir().unwrap();
        let first = staged_path(dir.path(), Path::new("/Users/me/Downloads/a.csv")).unwrap();
        let second = staged_path(dir.path(), Path::new("/Users/me/Downloads/a.csv")).unwrap();
        assert_eq!(first.file_name().unwrap(), "a.csv");
        assert_ne!(first, second);
        assert!(first.starts_with(dir.path()));
        let unnamed = staged_path(dir.path(), Path::new("")).unwrap();
        assert_eq!(unnamed.file_name().unwrap(), "download");
    }

    #[test]
    fn finished_downloads_find_their_staged_file() {
        let mut downloads = std::collections::HashMap::new();
        downloads.insert(
            "http://x/a".to_string(),
            vec![PathBuf::from("/s/1/a"), PathBuf::from("/s/2/a")],
        );
        assert_eq!(
            take_staged(&mut downloads, "http://x/a", None),
            Some(PathBuf::from("/s/1/a"))
        );
        assert_eq!(
            take_staged(&mut downloads, "http://x/a", Some(PathBuf::from("/s/2/a"))),
            Some(PathBuf::from("/s/2/a"))
        );
        assert!(downloads.is_empty());
        assert_eq!(take_staged(&mut downloads, "http://x/b", None), None);
    }

    #[test]
    fn only_loopback_hosts_are_ours() {
        assert!(is_loopback("127.0.0.1"));
        assert!(is_loopback("localhost"));
        assert!(is_loopback("[::1]"));
        assert!(!is_loopback("grafana.example.com"));
        assert!(!is_loopback("10.0.0.1"));
        assert!(is_loopback_url("https://127.0.0.1:52871/login"));
        assert!(!is_loopback_url("https://grafana.example.com/"));
        assert!(!is_loopback_url("not a url"));
    }

    #[test]
    fn pem_certificates_decode_to_der() {
        let pem = "-----BEGIN CERTIFICATE-----\nAAEC\nAw==\n-----END CERTIFICATE-----\n";
        assert_eq!(pem_to_der(pem), Some(vec![0, 1, 2, 3]));
        assert_eq!(pem_to_der("garbage"), None);
    }

    #[test]
    fn shortcuts_match_modifiers_and_key() {
        let shortcuts = vec![
            Keystroke::parse("ctrl-k").unwrap(),
            Keystroke::parse("ctrl-shift-tab").unwrap(),
        ];
        assert!(is_shortcut(
            &shortcuts,
            &Keystroke::parse("ctrl-k").unwrap()
        ));
        assert!(is_shortcut(
            &shortcuts,
            &Keystroke::parse("ctrl-shift-tab").unwrap()
        ));
        assert!(!is_shortcut(
            &shortcuts,
            &Keystroke::parse("ctrl-c").unwrap()
        ));
        assert!(!is_shortcut(&shortcuts, &Keystroke::parse("k").unwrap()));
    }

    #[test]
    fn storage_names_are_hex() {
        assert_eq!(storage_name(&[0xab; 16]), "ab".repeat(16));
    }
}
