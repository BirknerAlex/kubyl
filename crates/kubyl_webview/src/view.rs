//! The web-view tab (board 10): a toolbar (back, forward, reload, address bar with the
//! service chip and origin, zoom, open in browser, dev tools, more) over the page.
//!
//! The page is a native web view ([`crate::native`]) placed by [`crate::host`]. Everything
//! GPUI draws in its place (starting, errors, the certificate interstitial, the more menu)
//! hides the native view first.

use std::cell::RefCell;
use std::collections::{HashSet, VecDeque};
use std::path::PathBuf;
use std::rc::Rc;
use std::time::{Duration, Instant};

use futures::StreamExt as _;
use futures::channel::mpsc::{self, UnboundedSender};
use gpui::{
    AnyElement, App, AppContext as _, ClipboardItem, Context, Entity, EntityId, FocusHandle,
    Focusable, FontWeight, Global, Hsla, IntoElement, Keystroke, Render, SharedString, Task,
    Window, div, prelude::*, px,
};
use gpui_component::input::{Input, InputEvent, InputState};
use kubyl_core::{ClusterCaps, Notification, NotificationCenter, TabView};
use kubyl_kube::ConnectionManager;
use kubyl_resources::{ResourceSelection, Selected};
use kubyl_ui::{
    ActiveColors, Button, Chip, Colors, Icon, IconButton, IconName, ProdBadge, fonts, h_flex, u,
    v_flex,
};

use crate::cert::CertInfo;
use crate::forward::{ForwardStatus, WebForwards};
use crate::host::{Embedded, WebContent};
use crate::native::{
    EditCommand, NativeEvent, NativeOptions, NativeWebView, ParentWindow, Placement, Storage,
};
use crate::store::{self, OpenIn, PortKey, WebViewSettings};
use crate::target::{self, Scheme, TargetKind, WebTarget};
use crate::{
    ClearSiteData, Copy, CopyUrl, Cut, DevTools, FocusAddress, OpenInBrowser, Paste, Redo,
    ReloadPage, ResetPageZoom, SelectAll, StopForward, Undo, ZoomPageIn, ZoomPageOut,
};

/// How to open a web view.
#[derive(Clone, Debug, PartialEq)]
pub struct OpenRequest {
    pub target: WebTarget,
    /// Where to go: a path (`/graph`) or, for a page's new window, a full URL on the forward.
    pub path: Option<String>,
    pub scheme: Option<Scheme>,
    /// Detected from the port (`https` names, 443/8443); a remembered choice wins.
    pub detected: Scheme,
    /// The preset start path, when a preset fits.
    pub preset_path: Option<&'static str>,
    pub private: Option<bool>,
}

impl OpenRequest {
    pub fn new(target: WebTarget) -> Self {
        Self {
            target,
            path: None,
            scheme: None,
            detected: Scheme::Http,
            preset_path: None,
            private: None,
        }
    }
}

/// Requests waiting for the view factory (the workspace builds tabs through
/// [`kubyl_core::ViewRegistry`]).
#[derive(Default)]
pub(crate) struct PendingOpens(pub VecDeque<OpenRequest>);

impl Global for PendingOpens {}

/// Page zoom steps, like browsers.
const ZOOM_STEPS: &[f64] = &[
    0.5, 0.67, 0.75, 0.8, 0.9, 1.0, 1.1, 1.25, 1.5, 1.75, 2.0, 2.5, 3.0,
];

#[derive(Clone, Debug, PartialEq)]
enum Phase {
    /// Waiting for the forward.
    Starting,
    /// The page is (being) shown.
    Page,
    /// The forward failed, or the view couldn't be created.
    Failed(String),
    /// The cluster disconnected; the forward restarts when it's back.
    Disconnected,
    /// The server's certificate needs the user's decision.
    Certificate(CertInfo),
    /// Stopped after being in the background; restarts when shown.
    Idle,
    /// The web content process died.
    Crashed,
}

#[derive(Clone, Debug, PartialEq)]
enum DownloadState {
    Running,
    /// Waiting for the save dialog.
    Saving,
    Saved(PathBuf),
    Failed,
}

#[derive(Clone, Debug)]
struct Download {
    name: String,
    staged: Option<PathBuf>,
    url: String,
    state: DownloadState,
}

pub struct WebViewTab {
    target: WebTarget,
    key: Option<PortKey>,
    scheme: Scheme,
    private: bool,
    /// The page to load first (path, or a full URL on the forward).
    initial: String,
    focus: FocusHandle,
    content_focus: FocusHandle,
    address: Entity<InputState>,
    /// The address bar shows `url`'s path unless the user is editing it.
    editing_address: bool,
    phase: Phase,
    title: Option<String>,
    url: Option<String>,
    /// `http://127.0.0.1:<port>` of the running forward.
    origin: Option<String>,
    parent: Option<ParentWindow>,
    embedded: Option<Rc<Embedded>>,
    creating: bool,
    browser_opened: bool,
    events: UnboundedSender<NativeEvent>,
    zoom: f64,
    menu_open: bool,
    /// The page went to a host outside the forward (an app redirecting to its external URL).
    external: Option<String>,
    allowed_hosts: HashSet<String>,
    accepted: Rc<RefCell<Vec<[u8; 32]>>>,
    shortcuts: Rc<RefCell<Vec<Keystroke>>>,
    /// The keymap the shortcuts were read from.
    shortcuts_version: Option<gpui::KeymapVersion>,
    downloads: Vec<Download>,
    closing: bool,
    /// When the page was last seen on screen (idle stop).
    last_seen: Instant,
    /// Width of the address bar in the last frame: what fits in it.
    address_width: Rc<std::cell::Cell<f32>>,
    self_id: EntityId,
    _tasks: Vec<Task<()>>,
    _subscriptions: Vec<gpui::Subscription>,
}

impl WebViewTab {
    pub fn new(request: OpenRequest, window: &mut Window, cx: &mut Context<Self>) -> Self {
        let target = request.target.clone();
        let key = PortKey::of(&target, cx);
        let memory = key
            .as_ref()
            .map(|k| store::get(cx).port(k))
            .unwrap_or_default();
        let settings = WebViewSettings::get(cx).clone();
        let scheme = request.scheme.or(memory.scheme).unwrap_or(request.detected);
        let private = request
            .private
            .unwrap_or(memory.private || settings.private_by_default);
        let initial = request
            .path
            .clone()
            .or(memory.last_path.clone())
            .or(memory.start_path.clone())
            .or(request.preset_path.map(String::from))
            .unwrap_or_else(|| "/".into());
        let accepted = Rc::new(RefCell::new(
            key.as_ref()
                .map(|k| store::get(cx).accepted(k))
                .unwrap_or_default(),
        ));
        let address = cx.new(|cx| {
            InputState::new(window, cx).placeholder("/").default_value(
                if initial.starts_with("http") {
                    String::new()
                } else {
                    initial.clone()
                },
            )
        });
        let focus = cx.focus_handle();
        let content_focus = cx.focus_handle();
        let (events, mut rx) = mpsc::unbounded();

        let mut subscriptions = vec![
            cx.subscribe_in(
                &address,
                window,
                |this, _, event: &InputEvent, window, cx| match event {
                    InputEvent::Focus => this.editing_address = true,
                    InputEvent::Blur => {
                        this.editing_address = false;
                        this.sync_address(window, cx);
                    }
                    InputEvent::PressEnter { .. } => this.go_to_address(window, cx),
                    InputEvent::Change => {}
                },
            ),
            cx.on_focus(&content_focus, window, |this, window, cx| {
                this.content_focused(window, cx)
            }),
            cx.on_blur(&content_focus, window, |this, _, _| {
                if let Some(embedded) = &this.embedded {
                    embedded.native.blur();
                }
            }),
            cx.on_release(|this, cx| {
                let (target, id) = (this.target.clone(), this.self_id);
                if let Some(forwards) = WebForwards::try_global(cx) {
                    forwards.update(cx, |f, cx| f.release(&target, id, cx));
                }
            }),
        ];
        if let Some(forwards) = WebForwards::try_global(cx) {
            subscriptions.push(cx.observe_in(&forwards, window, |this, _, window, cx| {
                this.forward_changed(window, cx)
            }));
        }
        if let Some(connections) = ConnectionManager::try_global(cx) {
            subscriptions.push(cx.observe(&connections, |_, _, cx| cx.notify()));
        }

        let native_events = cx.spawn_in(window, async move |this, cx| {
            while let Some(event) = rx.next().await {
                if this
                    .update_in(cx, |this, window, cx| this.on_native(event, window, cx))
                    .is_err()
                {
                    break;
                }
            }
        });
        let idle = cx.spawn_in(window, async move |this, cx| {
            loop {
                cx.background_executor()
                    .timer(Duration::from_secs(30))
                    .await;
                if this.update(cx, |this, cx| this.check_idle(cx)).is_err() {
                    break;
                }
            }
        });

        let mut this = Self {
            target,
            key,
            scheme,
            private,
            initial,
            focus,
            content_focus,
            address,
            editing_address: false,
            phase: Phase::Starting,
            title: None,
            url: None,
            origin: None,
            parent: ParentWindow::of(window).ok(),
            embedded: None,
            creating: false,
            browser_opened: false,
            events,
            zoom: memory.zoom.unwrap_or(1.0),
            menu_open: false,
            external: None,
            allowed_hosts: HashSet::new(),
            accepted,
            shortcuts: Rc::default(),
            shortcuts_version: None,
            downloads: Vec::new(),
            closing: false,
            last_seen: Instant::now(),
            address_width: Rc::new(std::cell::Cell::new(f32::MAX)),
            self_id: cx.entity_id(),
            _tasks: vec![native_events, idle],
            _subscriptions: subscriptions,
        };
        this.hold(true, cx);
        this
    }

    pub fn target(&self) -> &WebTarget {
        &self.target
    }

    /// Asks the pane to close this tab (its forward was stopped from Active Sessions).
    pub fn request_close(&mut self, cx: &mut Context<Self>) {
        self.closing = true;
        cx.notify();
    }

    fn hold(&mut self, active: bool, cx: &mut Context<Self>) {
        let (target, scheme) = (self.target.clone(), self.scheme);
        let tab = crate::forward::holder(&cx.weak_entity());
        if let Some(forwards) = WebForwards::try_global(cx) {
            forwards.update(cx, |f, cx| f.hold(&target, tab, active, scheme, cx));
        }
    }

    fn caps(&self, cx: &App) -> ClusterCaps {
        ConnectionManager::try_global(cx)
            .map(|m| m.read(cx).caps(&self.target.cluster))
            .unwrap_or_default()
    }

    /// The forward changed: start the page, follow a new local port, or show why not.
    fn forward_changed(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if matches!(
            self.phase,
            Phase::Idle | Phase::Certificate(_) | Phase::Crashed
        ) {
            cx.notify();
            return;
        }
        let status = WebForwards::global(cx).read(cx).status(&self.target, cx);
        match status {
            None | Some(ForwardStatus::Starting) => {
                if self.embedded.is_none() {
                    self.set_phase(Phase::Starting);
                }
            }
            Some(ForwardStatus::Failed(error)) => self.set_phase(Phase::Failed(error)),
            Some(ForwardStatus::Disconnected) => {
                self.origin = None;
                self.set_phase(Phase::Disconnected);
            }
            Some(ForwardStatus::Ready { local_port, .. }) => {
                let origin = format!("{}://127.0.0.1:{local_port}", self.scheme.as_str());
                let changed = self.origin.as_deref() != Some(origin.as_str());
                let resumed = matches!(self.phase, Phase::Disconnected | Phase::Starting);
                self.origin = Some(origin.clone());
                self.set_phase(Phase::Page);
                if let Some(embedded) = &self.embedded {
                    if changed {
                        // A new local port (after a restart): same page, new origin.
                        let path = self
                            .url
                            .as_deref()
                            .and_then(target::remembered_path)
                            .unwrap_or_else(|| self.initial.clone());
                        embedded.native.load_url(&target::join(&origin, &path));
                    } else if resumed {
                        embedded.native.reload();
                    }
                } else if self.open_in_browser_mode(cx) {
                    if !self.browser_opened {
                        self.browser_opened = true;
                        cx.open_url(&self.start_url(&origin));
                    }
                } else if !self.creating {
                    self.create_native(origin, window, cx);
                }
            }
        }
        cx.notify();
    }

    fn set_phase(&mut self, phase: Phase) {
        if self.phase != phase {
            self.phase = phase;
        }
        if let Some(embedded) = &self.embedded {
            embedded.set_covered(self.covers_page());
        }
    }

    /// GPUI draws something where the page is.
    fn covers_page(&self) -> bool {
        self.phase != Phase::Page || self.menu_open
    }

    fn open_in_browser_mode(&self, cx: &App) -> bool {
        WebViewSettings::get(cx).open_in == OpenIn::Browser
            || self.parent.is_none()
            || crate::native::supported().is_err()
    }

    fn start_url(&self, origin: &str) -> String {
        if self.initial.starts_with("http://") || self.initial.starts_with("https://") {
            // A new window's URL: same page, on this forward's origin.
            let path = target::remembered_path(&self.initial).unwrap_or_else(|| "/".into());
            target::join(origin, &path)
        } else {
            target::join(origin, &self.initial)
        }
    }

    /// Creates the platform view. Runs outside the window update: creating a WebView2
    /// controller pumps the Win32 message loop, which must not see GPUI's `App` borrowed.
    fn create_native(&mut self, origin: String, window: &mut Window, cx: &mut Context<Self>) {
        let Some(parent) = self.parent else {
            return;
        };
        self.creating = true;
        let data_dir = data_dir();
        let options = NativeOptions {
            url: self.start_url(&origin),
            title: format!("{} · {}", self.target, cluster_name(&self.target, cx)),
            storage: match (&self.key, self.private) {
                (Some(key), false) => Storage::Isolated {
                    id: key.storage_id(),
                },
                _ => Storage::Private,
            },
            data_dir: data_dir.clone(),
            staging_dir: data_dir.join("downloads"),
            devtools: WebViewSettings::devtools(cx),
            zoom: self.zoom,
            accepted_certs: self.accepted.clone(),
            shortcuts: self.shortcuts.clone(),
        };
        let events = self.events.clone();
        cx.spawn_in(window, async move |this, cx| {
            let started = Instant::now();
            let result = NativeWebView::create(&parent, options, events);
            tracing::debug!(elapsed = ?started.elapsed(), ok = result.is_ok(), "web view created");
            this.update_in(cx, |this, window, cx| {
                this.creating = false;
                match result {
                    Ok(native) => {
                        let embedded = Embedded::new(native, window.window_handle());
                        crate::start_pump(cx);
                        embedded.set_covered(this.covers_page());
                        if this.content_focus.is_focused(window) {
                            embedded.native.focus();
                        }
                        this.embedded = Some(embedded);
                    }
                    Err(err) => {
                        tracing::warn!("web view: {err:#}");
                        this.set_phase(Phase::Failed(format!(
                            "Couldn't create the web view: {err:#}"
                        )));
                    }
                }
                cx.notify();
            })
            .ok();
        })
        .detach();
    }

    fn on_native(&mut self, event: NativeEvent, window: &mut Window, cx: &mut Context<Self>) {
        match event {
            NativeEvent::TitleChanged(title) => {
                tracing::debug!(target = %self.target, title, "page title");
                let title = title.trim().to_string();
                self.title = (!title.is_empty()).then_some(title);
                if let Some(embedded) = &self.embedded {
                    embedded.native.set_window_title(&self.tab_title(cx));
                }
            }
            NativeEvent::LoadStarted(url) | NativeEvent::LoadFinished(url) => {
                self.page_url_changed(url, window, cx)
            }
            NativeEvent::NewWindow(url) => self.new_window(url, window, cx),
            NativeEvent::DownloadStarted { url, staged } => {
                let name = staged
                    .file_name()
                    .map(|n| n.to_string_lossy().into_owned())
                    .unwrap_or_else(|| "download".into());
                self.downloads.push(Download {
                    name,
                    staged: Some(staged),
                    url,
                    state: DownloadState::Running,
                });
            }
            NativeEvent::DownloadFinished { url, staged, ok } => {
                self.download_finished(url, staged, ok, cx)
            }
            NativeEvent::CertificateRejected { chain } => {
                if let Some(leaf) = chain.first() {
                    self.set_phase(Phase::Certificate(CertInfo::parse(leaf)));
                }
            }
            NativeEvent::Focused => {
                self.menu_open = false;
                self.content_focus.focus(window, cx);
            }
            NativeEvent::Shortcut(keystroke) => {
                self.content_focus.focus(window, cx);
                window.dispatch_keystroke(keystroke, cx);
            }
            NativeEvent::WindowClosed => self.request_close(cx),
            NativeEvent::Crashed => self.set_phase(Phase::Crashed),
        }
        cx.notify();
    }

    fn page_url_changed(&mut self, url: String, window: &mut Window, cx: &mut Context<Self>) {
        let on_forward = self
            .origin
            .as_deref()
            .is_some_and(|origin| target::same_origin(&url, origin));
        if on_forward {
            self.external = None;
            if let (Some(key), Some(path)) = (&self.key, target::remembered_path(&url))
                && !self.private
            {
                let key = key.clone();
                store::update(cx, |state| {
                    state.update_port(&key, |m| m.last_path = Some(path))
                });
            }
        } else if let Ok(parsed) = url::Url::parse(&url)
            && matches!(parsed.scheme(), "http" | "https")
            && let Some(host) = parsed.host_str()
            && !self.allowed_hosts.contains(host)
        {
            self.external = Some(url.clone());
        }
        self.url = Some(url);
        self.sync_address(window, cx);
    }

    /// Shows the page's path in the address bar (unless the user is typing there).
    fn sync_address(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.editing_address {
            return;
        }
        let path = match (&self.url, &self.origin) {
            (Some(url), Some(origin)) if target::same_origin(url, origin) => {
                let parsed = url::Url::parse(url).ok();
                parsed
                    .map(|u| {
                        let mut path = u.path().to_string();
                        if let Some(query) = u.query() {
                            path.push('?');
                            path.push_str(query);
                        }
                        if let Some(fragment) = u.fragment() {
                            path.push('#');
                            path.push_str(fragment);
                        }
                        path
                    })
                    .unwrap_or_default()
            }
            (Some(url), _) => url.clone(),
            (None, _) => return,
        };
        self.address.update(cx, |input, cx| {
            if input.value() != path.as_str() {
                input.set_value(path, window, cx);
            }
        });
    }

    fn go_to_address(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let typed = self.address.read(cx).value().trim().to_string();
        let Some(origin) = self.origin.clone() else {
            return;
        };
        // The host is locked to the forward: only paths are accepted.
        let url = match url::Url::parse(&typed) {
            Ok(parsed) if matches!(parsed.scheme(), "http" | "https") => target::join(
                &origin,
                &target::remembered_path(&typed).unwrap_or_default(),
            ),
            _ => target::join(&origin, &typed),
        };
        if let Some(embedded) = &self.embedded {
            embedded.native.load_url(&url);
        } else if self.open_in_browser_mode(cx) {
            cx.open_url(&url);
        }
        self.editing_address = false;
        self.content_focus.focus(window, cx);
    }

    /// `target=_blank` / `window.open`: pages on this forward open in a new Kubyl tab (sharing
    /// the forward), anything else in the system browser.
    fn new_window(&mut self, url: String, window: &mut Window, cx: &mut Context<Self>) {
        let on_forward = self
            .origin
            .as_deref()
            .is_some_and(|origin| target::same_origin(&url, origin));
        if on_forward {
            let mut request = OpenRequest::new(self.target.clone());
            request.path = target::remembered_path(&url).or(Some(url));
            request.scheme = Some(self.scheme);
            request.private = Some(self.private);
            crate::open_in(request, window, cx);
        } else if url.starts_with("http://") || url.starts_with("https://") {
            cx.open_url(&url);
        }
    }

    fn download_finished(
        &mut self,
        url: String,
        staged: Option<PathBuf>,
        ok: bool,
        cx: &mut Context<Self>,
    ) {
        let index = self.downloads.iter().position(|d| {
            d.state == DownloadState::Running && (d.staged == staged || d.url == url)
        });
        let Some(index) = index else {
            return;
        };
        if !ok {
            self.downloads[index].state = DownloadState::Failed;
            return;
        }
        if staged.is_some() {
            self.downloads[index].staged = staged;
        }
        self.save_download(index, cx);
    }

    /// Asks where to keep a finished download (Kubyl's save dialog), then moves it there.
    fn save_download(&mut self, index: usize, cx: &mut Context<Self>) {
        let Some(download) = self.downloads.get_mut(index) else {
            return;
        };
        let Some(staged) = download.staged.clone() else {
            return;
        };
        download.state = DownloadState::Saving;
        let directory = dirs::download_dir()
            .or_else(dirs::home_dir)
            .unwrap_or_else(|| PathBuf::from("."));
        let path = cx.prompt_for_new_path(&directory, Some(&download.name));
        cx.spawn(async move |this, cx| {
            let chosen = match path.await {
                Ok(Ok(Some(path))) => Some(path),
                _ => None,
            };
            let result = match &chosen {
                Some(path) => {
                    let (from, to) = (staged.clone(), path.clone());
                    cx.background_executor()
                        .spawn(async move { move_file(&from, &to) })
                        .await
                        .map(|()| Some(to_owned(path)))
                }
                None => {
                    std::fs::remove_file(&staged).ok();
                    Ok(None)
                }
            };
            this.update(cx, |this, cx| {
                if let Some(download) = this
                    .downloads
                    .iter_mut()
                    .find(|d| d.staged.as_ref() == Some(&staged))
                {
                    match &result {
                        Ok(Some(path)) => download.state = DownloadState::Saved(path.clone()),
                        Ok(None) => {}
                        Err(_) => download.state = DownloadState::Failed,
                    }
                }
                this.downloads.retain(|d| {
                    !(d.staged.as_ref() == Some(&staged) && d.state == DownloadState::Saving)
                });
                match result {
                    Ok(Some(path)) => NotificationCenter::push(
                        cx,
                        Notification::success(format!("Saved {}", path.display())),
                    ),
                    Ok(None) => {}
                    Err(err) => NotificationCenter::push(
                        cx,
                        Notification::error(format!("Couldn't save the download: {err}")),
                    ),
                }
                cx.notify();
            })
            .ok();
        })
        .detach();
    }

    /// The page got GPUI's focus: give it the keyboard, show its service in the details dock
    /// and learn which shortcuts are Kubyl's in this context.
    fn content_focused(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if let Some(embedded) = &self.embedded {
            embedded.native.focus();
        }
        self.refresh_shortcuts(window, cx);
        let caps = self.caps(cx);
        let kind = match self.target.kind {
            TargetKind::Service => "Service",
            TargetKind::Pod => "Pod",
        };
        let already = ResourceSelection::global(cx)
            .primary()
            .is_some_and(|s| s.target == self.target.object());
        if !already {
            ResourceSelection::set(
                cx,
                ResourceSelection {
                    items: vec![Selected {
                        target: self.target.object(),
                        kind: kind.into(),
                        object: None,
                        store: None,
                    }],
                    caps,
                },
            );
        }
    }

    /// Learns which keystrokes Kubyl binds in the page's key context (while it's focused, so
    /// the context stack is the page's), again whenever the keymap changes.
    fn refresh_shortcuts(&mut self, window: &Window, cx: &App) {
        if !self.content_focus.is_focused(window) {
            return;
        }
        let version = cx.key_bindings().borrow().version();
        if self.shortcuts_version == Some(version) && !self.shortcuts.borrow().is_empty() {
            return;
        }
        self.shortcuts_version = Some(version);
        *self.shortcuts.borrow_mut() = kubyl_shortcuts(window, cx);
    }

    /// Stops the forward of a tab that stayed in the background (setting), and restarts it
    /// when the tab shows again.
    fn check_idle(&mut self, cx: &mut Context<Self>) {
        let shown = self.embedded.as_ref().is_some_and(|e| e.is_shown());
        if shown || self.phase != Phase::Page || self.open_in_browser_mode(cx) {
            self.last_seen = Instant::now();
            return;
        }
        let minutes = WebViewSettings::get(cx).idle_stop_minutes;
        if minutes > 0 && self.last_seen.elapsed() > Duration::from_secs(u64::from(minutes) * 60) {
            self.set_phase(Phase::Idle);
            self.hold(false, cx);
            cx.notify();
        }
    }

    /// Rendering means the tab is on screen: an idle tab wakes up.
    fn wake_if_idle(&mut self, cx: &mut Context<Self>) {
        if self.phase == Phase::Idle {
            self.last_seen = Instant::now();
            self.phase = Phase::Starting;
            self.hold(true, cx);
        }
    }

    fn tab_title(&self, _cx: &App) -> String {
        let page = self
            .title
            .clone()
            .or_else(|| {
                self.url
                    .as_deref()
                    .and_then(target::remembered_path)
                    .filter(|p| p != "/")
            })
            .filter(|page| page != &self.target.name);
        match page {
            Some(page) => format!("{} · {page}", self.target.name),
            None => self.target.name.clone(),
        }
    }

    // --- Actions ------------------------------------------------------------------------

    fn with_native(&self, f: impl FnOnce(&NativeWebView)) {
        if let Some(embedded) = &self.embedded {
            f(&embedded.native);
        }
    }

    fn reload(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        match self.phase {
            Phase::Page => self.with_native(|n| n.reload()),
            Phase::Failed(_) | Phase::Crashed => {
                self.embedded = None;
                self.phase = Phase::Starting;
                self.hold(true, cx);
                self.forward_changed(window, cx);
            }
            _ => {}
        }
    }

    fn set_zoom(&mut self, zoom: f64, cx: &mut Context<Self>) {
        self.zoom = zoom.clamp(ZOOM_STEPS[0], ZOOM_STEPS[ZOOM_STEPS.len() - 1]);
        let zoom = self.zoom;
        self.with_native(|n| n.set_zoom(zoom));
        if let Some(key) = self.key.clone() {
            store::update(cx, |state| {
                state.update_port(&key, |m| {
                    m.zoom = ((zoom - 1.0).abs() > f64::EPSILON).then_some(zoom)
                })
            });
        }
        cx.notify();
    }

    fn zoom_step(&mut self, up: bool, cx: &mut Context<Self>) {
        let next = if up {
            ZOOM_STEPS.iter().copied().find(|z| *z > self.zoom + 0.001)
        } else {
            ZOOM_STEPS
                .iter()
                .rev()
                .copied()
                .find(|z| *z < self.zoom - 0.001)
        };
        if let Some(zoom) = next {
            self.set_zoom(zoom, cx);
        }
    }

    fn current_url(&self) -> Option<String> {
        self.embedded
            .as_ref()
            .and_then(|e| e.native.url())
            .or_else(|| self.url.clone())
            .or_else(|| self.origin.as_ref().map(|o| self.start_url(o)))
    }

    fn open_in_browser(&mut self, cx: &mut Context<Self>) {
        if let Some(url) = self.current_url() {
            cx.open_url(&url);
        }
    }

    fn copy_url(&mut self, cx: &mut Context<Self>) {
        if let Some(url) = self.current_url() {
            cx.write_to_clipboard(ClipboardItem::new_string(url.clone()));
            NotificationCenter::push(cx, Notification::info(format!("Copied {url}")));
        }
    }

    fn clear_site_data(&mut self, cx: &mut Context<Self>) {
        self.with_native(|n| n.clear_browsing_data());
        if let Some(key) = self.key.clone() {
            store::update(cx, |state| {
                state.forget_certs(&key);
                state.update_port(&key, |m| m.last_path = None);
            });
        }
        self.accepted.borrow_mut().clear();
        self.with_native(|n| n.reload());
        NotificationCenter::push(
            cx,
            Notification::info(format!("Cleared the site data of {}", self.target.name)),
        );
    }

    /// "Clear site data" from the details section.
    pub fn clear_site_data_from_details(&mut self, cx: &mut Context<Self>) {
        self.clear_site_data(cx);
    }

    /// Reopens the page in a private (or back in the isolated) session.
    pub fn set_private(&mut self, private: bool, window: &mut Window, cx: &mut Context<Self>) {
        if self.private == private {
            return;
        }
        self.private = private;
        if let Some(key) = self.key.clone() {
            store::update(cx, |state| state.update_port(&key, |m| m.private = private));
        }
        if let Some(url) = self.current_url()
            && let Some(path) = target::remembered_path(&url)
        {
            self.initial = path;
        }
        self.embedded = None;
        self.forward_changed(window, cx);
    }

    fn set_scheme(&mut self, scheme: Scheme, window: &mut Window, cx: &mut Context<Self>) {
        if self.scheme == scheme {
            return;
        }
        self.scheme = scheme;
        if let Some(key) = self.key.clone() {
            store::update(cx, |state| {
                state.update_port(&key, |m| m.scheme = Some(scheme))
            });
        }
        self.origin = None;
        self.forward_changed(window, cx);
    }

    fn proceed(&mut self, cert: &CertInfo, window: &mut Window, cx: &mut Context<Self>) {
        self.accepted.borrow_mut().push(cert.sha256);
        if let Some(key) = self.key.clone() {
            let sha = cert.sha256;
            store::update(cx, |state| state.accept(key, &sha));
        }
        self.phase = Phase::Page;
        if let Some(embedded) = &self.embedded {
            embedded.set_covered(false);
            let url = self
                .url
                .clone()
                .filter(|u| {
                    self.origin
                        .as_deref()
                        .is_some_and(|o| target::same_origin(u, o))
                })
                .or_else(|| self.origin.as_ref().map(|o| self.start_url(o)));
            if let Some(url) = url {
                embedded.native.load_url(&url);
            }
        }
        self.forward_changed(window, cx);
    }

    fn toggle_menu(&mut self, cx: &mut Context<Self>) {
        self.menu_open = !self.menu_open;
        if let Some(embedded) = &self.embedded {
            embedded.set_covered(self.covers_page());
        }
        cx.notify();
    }

    fn close_menu(&mut self, cx: &mut Context<Self>) {
        if self.menu_open {
            self.toggle_menu(cx);
        }
    }

    // --- Rendering ----------------------------------------------------------------------

    fn render_toolbar(
        &self,
        caps: &ClusterCaps,
        cx: &mut Context<Self>,
    ) -> impl IntoElement + use<> {
        let colors = cx.colors().clone();
        let (can_back, can_forward) = self
            .embedded
            .as_ref()
            .filter(|_| self.phase == Phase::Page)
            .map(|e| (e.native.can_go_back(), e.native.can_go_forward()))
            .unwrap_or((false, false));
        let status_color = match &self.phase {
            Phase::Page => colors.green,
            Phase::Starting | Phase::Idle => colors.text_dim,
            Phase::Disconnected | Phase::Certificate(_) => colors.yellow,
            Phase::Failed(_) | Phase::Crashed => colors.red,
        };
        let origin = self
            .origin
            .clone()
            .unwrap_or_else(|| format!("{}://127.0.0.1:…", self.scheme.as_str()));
        let session = if self.private {
            (IconName::EyeOff, "private session")
        } else {
            (IconName::Lock, "isolated session")
        };
        let zoom_label = format!("{}%", (self.zoom * 100.0).round() as i64);
        // Narrow address bars drop the session label, then the origin, so the path keeps
        // room (widths estimated from the monospace text: ~0.6 em per character).
        let width = self.address_width.get();
        let chip = short_label(&self.target).chars().count() as f32 * 6.6 + 40.0;
        let origin_width = origin.chars().count() as f32 * 7.2 + 8.0;
        let badges =
            if caps.production { 44.0 } else { 0.0 } + if caps.read_only { 90.0 } else { 0.0 };
        let room = width - chip - badges - 16.0 - 180.0;
        let show_origin = room > origin_width + 24.0;
        let show_session = room > origin_width + 24.0 + 120.0;
        let measured = self.address_width.clone();
        let entity = cx.entity().downgrade();
        h_flex()
            .flex_none()
            .h(u(42.0))
            .px(u(8.0))
            .gap(u(4.0))
            .border_b_1()
            .border_color(colors.border)
            .bg(colors.panel)
            .child(nav_button(
                "back",
                IconName::ArrowLeft,
                can_back,
                cx.listener(|this, _, _, _| this.with_native(|n| n.back())),
            ))
            .child(nav_button(
                "forward",
                IconName::ArrowRight,
                can_forward,
                cx.listener(|this, _, _, _| this.with_native(|n| n.forward())),
            ))
            .child(
                IconButton::new("reload", IconName::RefreshCw)
                    .icon_size(13.0)
                    .on_click(cx.listener(|this, _, window, cx| this.reload(window, cx))),
            )
            .child(
                h_flex()
                    .id("address")
                    .relative()
                    .flex_1()
                    .min_w_0()
                    .overflow_hidden()
                    .child(
                        gpui::canvas(
                            move |bounds, _, cx| {
                                let width = f32::from(bounds.size.width);
                                let old = measured.replace(width);
                                if (old - width).abs() > 8.0 {
                                    entity.update(cx, |_, cx| cx.notify()).ok();
                                }
                            },
                            |_, _, _, _| {},
                        )
                        .absolute()
                        .size_full(),
                    )
                    .h(u(28.0))
                    .mx(u(6.0))
                    .px(u(8.0))
                    .gap(u(8.0))
                    .rounded(u(5.0))
                    .border_1()
                    .border_color(colors.border)
                    .bg(colors.input_background)
                    .when(caps.production, |this| this.child(ProdBadge))
                    .when(caps.read_only, |this| {
                        this.child(
                            Chip::new("read-only")
                                .icon(IconName::Eye)
                                .text_color(colors.yellow),
                        )
                    })
                    .child(
                        Chip::new(short_label(&self.target))
                            .mono()
                            .icon(IconName::Link)
                            .text_color(colors.text)
                            .dot(status_color),
                    )
                    .when(show_origin, |this| {
                        this.child(
                            div()
                                .flex_none()
                                .font_family(fonts::MONO)
                                .text_size(u(12.0))
                                .text_color(colors.text_dim)
                                .child(origin),
                        )
                    })
                    .child(
                        div()
                            .flex_1()
                            .min_w(u(120.0))
                            .when(show_origin, |this| this.ml(u(-8.0)))
                            .font_family(fonts::MONO)
                            .text_size(u(12.0))
                            .child(Input::new(&self.address).appearance(false)),
                    )
                    .child(
                        h_flex()
                            .flex_none()
                            .gap(u(4.0))
                            .text_size(u(11.5))
                            .text_color(colors.text_dim)
                            .child(Icon::new(session.0).size(11.0).color(colors.text_dim))
                            .when(show_session, |this| this.child(session.1)),
                    ),
            )
            .child(
                div()
                    .id("zoom")
                    .cursor_pointer()
                    .child(Chip::new(zoom_label))
                    .on_click(cx.listener(|this, _, _, cx| this.set_zoom(1.0, cx))),
            )
            .child(
                Button::new("open-browser")
                    .ghost()
                    .icon(IconName::Globe)
                    .label("Open in browser")
                    .on_click(cx.listener(|this, _, _, cx| this.open_in_browser(cx))),
            )
            .when(WebViewSettings::devtools(cx), |this| {
                this.child(
                    IconButton::new("devtools", IconName::Code)
                        .icon_size(14.0)
                        .on_click(
                            cx.listener(|this, _, _, _| this.with_native(|n| n.open_devtools())),
                        ),
                )
            })
            .child(
                IconButton::new("more", IconName::Ellipsis)
                    .toggled(self.menu_open)
                    .on_click(cx.listener(|this, _, _, cx| this.toggle_menu(cx))),
            )
    }

    fn render_menu(&self, cx: &mut Context<Self>) -> impl IntoElement + use<> {
        let colors = cx.colors().clone();
        let item = |id: &'static str, label: String, cx: &mut Context<Self>| {
            div()
                .id(id)
                .px(u(10.0))
                .py(u(5.0))
                .rounded(u(4.0))
                .cursor_pointer()
                .hover(|s| s.bg(colors.hover))
                .child(label)
                .on_mouse_down(
                    gpui::MouseButton::Left,
                    cx.listener(|_, _, _, cx| cx.stop_propagation()),
                )
        };
        let separator = || div().my(u(4.0)).h(px(1.0)).bg(colors.border);
        let scheme_label = match self.scheme {
            Scheme::Http => "Use HTTPS",
            Scheme::Https => "Use HTTP",
        };
        v_flex()
            .id("webview-menu")
            .occlude()
            .absolute()
            .top(u(40.0))
            .right(u(8.0))
            .w(u(240.0))
            .p(u(4.0))
            .rounded(u(6.0))
            .border_1()
            .border_color(colors.border)
            .bg(colors.elevated)
            .shadow_lg()
            .text_size(u(12.5))
            .text_color(colors.text)
            .on_mouse_down_out(cx.listener(|this, _, _, cx| this.close_menu(cx)))
            .child(item("m-reload", "Reload".into(), cx).on_click(cx.listener(
                |this, _, window, cx| {
                    this.close_menu(cx);
                    this.reload(window, cx)
                },
            )))
            .child(
                item("m-copy", "Copy URL".into(), cx).on_click(cx.listener(|this, _, _, cx| {
                    this.close_menu(cx);
                    this.copy_url(cx)
                })),
            )
            .child(
                item("m-new-tab", "Open in a new tab".into(), cx).on_click(cx.listener(
                    |this, _, window, cx| {
                        this.close_menu(cx);
                        if let Some(url) = this.current_url() {
                            this.new_window(url, window, cx);
                        }
                    },
                )),
            )
            .child(separator())
            .child(
                item("m-zoom-in", "Zoom in".into(), cx)
                    .on_click(cx.listener(|this, _, _, cx| this.zoom_step(true, cx))),
            )
            .child(
                item("m-zoom-out", "Zoom out".into(), cx)
                    .on_click(cx.listener(|this, _, _, cx| this.zoom_step(false, cx))),
            )
            .child(separator())
            .child(
                item("m-scheme", scheme_label.into(), cx).on_click(cx.listener(
                    |this, _, window, cx| {
                        this.close_menu(cx);
                        let scheme = match this.scheme {
                            Scheme::Http => Scheme::Https,
                            Scheme::Https => Scheme::Http,
                        };
                        this.set_scheme(scheme, window, cx);
                    },
                )),
            )
            .child(
                item("m-start", "Start here next time".into(), cx).on_click(cx.listener(
                    |this, _, _, cx| {
                        this.close_menu(cx);
                        this.remember_start_path(cx);
                    },
                )),
            )
            .child(
                item(
                    "m-private",
                    if self.private {
                        "Leave private session".into()
                    } else {
                        "Private session".into()
                    },
                    cx,
                )
                .on_click(cx.listener(|this, _, window, cx| {
                    this.close_menu(cx);
                    let private = !this.private;
                    this.set_private(private, window, cx);
                })),
            )
            .child(
                item("m-clear", "Clear site data".into(), cx).on_click(cx.listener(
                    |this, _, _, cx| {
                        this.close_menu(cx);
                        this.clear_site_data(cx)
                    },
                )),
            )
            .child(separator())
            .child(
                item("m-stop", "Stop forward and close tabs".into(), cx)
                    .text_color(colors.red)
                    .on_click(cx.listener(|this, _, _, cx| {
                        let target = this.target.clone();
                        WebForwards::global(cx).update(cx, |f, cx| f.stop_and_close(&target, cx));
                    })),
            )
    }

    fn remember_start_path(&mut self, cx: &mut Context<Self>) {
        let Some(path) = self
            .current_url()
            .as_deref()
            .and_then(target::remembered_path)
        else {
            return;
        };
        if let Some(key) = self.key.clone() {
            store::update(cx, |state| {
                state.update_port(&key, |m| m.start_path = Some(path.clone()))
            });
            NotificationCenter::push(
                cx,
                Notification::info(format!("{} starts at {path} from now on", self.target)),
            );
        }
    }

    fn render_banner(&self, cx: &mut Context<Self>) -> Option<AnyElement> {
        let url = self.external.clone()?;
        let colors = cx.colors().clone();
        let host = url::Url::parse(&url)
            .ok()
            .and_then(|u| u.host_str().map(String::from))
            .unwrap_or_default();
        Some(
            h_flex()
                .flex_none()
                .px(u(12.0))
                .py(u(6.0))
                .gap(u(8.0))
                .border_b_1()
                .border_color(colors.border)
                .bg(colors.subheader_background)
                .text_size(u(12.5))
                .child(
                    Icon::new(IconName::TriangleAlert)
                        .size(13.0)
                        .color(colors.yellow),
                )
                .child(div().flex_1().min_w_0().truncate().child(format!(
                    "This app went to {url} (probably its external URL)."
                )))
                .child(
                    Button::new("external-open")
                        .label("Open in browser")
                        .on_click(cx.listener(move |_, _, _, cx| cx.open_url(&url))),
                )
                .child(
                    Button::new("external-back")
                        .label("Back")
                        .on_click(cx.listener(|this, _, _, cx| {
                            this.external = None;
                            this.with_native(|n| n.back());
                            cx.notify();
                        })),
                )
                .child(
                    Button::new("external-stay")
                        .ghost()
                        .label("Stay")
                        .on_click(cx.listener(move |this, _, _, cx| {
                            this.allowed_hosts.insert(host.clone());
                            this.external = None;
                            cx.notify();
                        })),
                )
                .into_any_element(),
        )
    }

    fn render_content(&self, window: &mut Window, cx: &mut Context<Self>) -> AnyElement {
        let colors = cx.colors().clone();
        let message = |icon: IconName, title: String, detail: Option<String>| {
            v_flex()
                .size_full()
                .items_center()
                .justify_center()
                .gap(u(8.0))
                .px(u(24.0))
                .child(Icon::new(icon).size(22.0).color(colors.text_dim))
                .child(
                    div()
                        .text_color(colors.text)
                        .font_weight(FontWeight::MEDIUM)
                        .child(title),
                )
                .when_some(detail, |this, detail| {
                    this.child(
                        div()
                            .max_w(u(560.0))
                            .text_center()
                            .text_size(u(12.5))
                            .text_color(colors.text_dim)
                            .child(detail),
                    )
                })
        };
        match &self.phase {
            Phase::Starting => message(
                IconName::Link,
                format!("Starting a port-forward to {}…", self.target),
                Some("Loopback only · not saved · stops when the last tab closes".into()),
            )
            .into_any_element(),
            Phase::Failed(error) => message(
                IconName::CircleX,
                format!("Couldn't open {}", self.target),
                Some(error.clone()),
            )
            .child(
                Button::new("retry")
                    .label("Try again")
                    .on_click(cx.listener(|this, _, window, cx| this.reload(window, cx))),
            )
            .into_any_element(),
            Phase::Disconnected => message(
                IconName::Cloud,
                "The cluster disconnected".into(),
                Some("The web view comes back when the cluster reconnects.".into()),
            )
            .into_any_element(),
            Phase::Idle => message(
                IconName::Pause,
                "Paused in the background".into(),
                Some("The port-forward stopped to save resources; it restarts now.".into()),
            )
            .into_any_element(),
            Phase::Crashed => message(IconName::TriangleAlert, "The page crashed".into(), None)
                .child(
                    Button::new("reload-crashed")
                        .label("Reload")
                        .on_click(cx.listener(|this, _, window, cx| this.reload(window, cx))),
                )
                .into_any_element(),
            Phase::Certificate(cert) => self.render_certificate(cert, cx),
            Phase::Page => {
                if self.open_in_browser_mode(cx) {
                    return self.render_browser_session(cx);
                }
                match &self.embedded {
                    Some(embedded) if embedded.native.placement() == Placement::Window => message(
                        IconName::Maximize,
                        "The page is open in its own window".into(),
                        Some(
                            "Wayland can't embed web views in Kubyl's window. Closing this \
                                 tab closes the window and stops the forward."
                                .into(),
                        ),
                    )
                    .child(
                        Button::new("raise")
                            .label("Show the window")
                            .on_click(cx.listener(|this, _, _, _| this.with_native(|n| n.focus()))),
                    )
                    .into_any_element(),
                    Some(embedded) => div()
                        .size_full()
                        .flex()
                        .bg(colors.background)
                        .child(WebContent::new(embedded.clone()))
                        .into_any_element(),
                    None => {
                        let _ = window;
                        message(IconName::Globe, format!("Loading {}…", self.target), None)
                            .into_any_element()
                    }
                }
            }
        }
    }

    fn render_browser_session(&self, cx: &mut Context<Self>) -> AnyElement {
        let colors = cx.colors().clone();
        let url = self
            .origin
            .as_ref()
            .map(|o| self.start_url(o))
            .unwrap_or_default();
        v_flex()
            .size_full()
            .items_center()
            .justify_center()
            .gap(u(10.0))
            .child(Icon::new(IconName::Globe).size(22.0).color(colors.accent))
            .child(
                div()
                    .text_color(colors.text)
                    .font_weight(FontWeight::MEDIUM)
                    .child(format!("{} is open in your browser", self.target)),
            )
            .child(
                div()
                    .font_family(fonts::MONO)
                    .text_size(u(12.0))
                    .text_color(colors.text_dim)
                    .child(url.clone()),
            )
            .child(
                h_flex()
                    .gap(u(6.0))
                    .child(Button::new("browser-again").label("Open again").on_click({
                        let url = url.clone();
                        move |_, _, cx| cx.open_url(&url)
                    }))
                    .child(
                        Button::new("browser-copy")
                            .ghost()
                            .label("Copy URL")
                            .on_click(cx.listener(|this, _, _, cx| this.copy_url(cx))),
                    ),
            )
            .child(
                div()
                    .text_size(u(12.0))
                    .text_color(colors.text_faint)
                    .child("The forward stays open while this tab is open."),
            )
            .into_any_element()
    }

    fn render_certificate(&self, cert: &CertInfo, cx: &mut Context<Self>) -> AnyElement {
        let colors = cx.colors().clone();
        let row = |label: &'static str, value: String| {
            h_flex()
                .gap(u(12.0))
                .items_start()
                .child(
                    div()
                        .flex_none()
                        .w(u(110.0))
                        .text_color(colors.text_dim)
                        .child(label),
                )
                .child(
                    div()
                        .flex_1()
                        .min_w_0()
                        .font_family(fonts::MONO)
                        .text_size(u(12.0))
                        .child(value),
                )
        };
        let mut problems = Vec::new();
        if cert.self_signed {
            problems.push("it is self-signed");
        } else {
            problems.push("its issuer isn't trusted here");
        }
        if cert.expired {
            problems.push("it has expired");
        }
        if cert.not_yet_valid {
            problems.push("it isn't valid yet");
        }
        let proceed_cert = cert.clone();
        v_flex()
            .id("certificate")
            .size_full()
            .overflow_y_scroll()
            .items_center()
            .pt(u(48.0))
            .px(u(24.0))
            .child(
                v_flex()
                    .w_full()
                    .max_w(u(640.0))
                    .gap(u(14.0))
                    .child(
                        h_flex()
                            .gap(u(10.0))
                            .child(Icon::new(IconName::Shield).size(22.0).color(colors.yellow))
                            .child(
                                div()
                                    .text_size(u(16.0))
                                    .font_weight(FontWeight::SEMIBOLD)
                                    .child(format!(
                                        "{} uses a certificate Kubyl can't verify",
                                        self.target
                                    )),
                            ),
                    )
                    .child(div().text_color(colors.text_muted).child(format!(
                        "The page is served over HTTPS through a port-forward on 127.0.0.1, and \
                         {}. Proceed only if you expect this certificate; Kubyl remembers the \
                         decision for this service port only.",
                        problems.join(" and ")
                    )))
                    .child(
                        v_flex()
                            .gap(u(6.0))
                            .p(u(12.0))
                            .rounded(u(6.0))
                            .border_1()
                            .border_color(colors.border)
                            .bg(colors.panel)
                            .child(row("Subject", cert.subject.clone()))
                            .child(row("Issuer", cert.issuer.clone()))
                            .child(row(
                                "Valid",
                                format!("{} → {}", cert.not_before, cert.not_after),
                            ))
                            .when(!cert.names.is_empty(), |this| {
                                this.child(row("Names", cert.names.join(", ")))
                            })
                            .child(row("SHA-256", cert.fingerprint())),
                    )
                    .child(
                        h_flex()
                            .gap(u(8.0))
                            .child(
                                Button::new("cert-proceed")
                                    .danger()
                                    .label(format!("Proceed to {}", self.target))
                                    .on_click(cx.listener(move |this, _, window, cx| {
                                        this.proceed(&proceed_cert, window, cx)
                                    })),
                            )
                            .child(
                                Button::new("cert-close")
                                    .label("Close tab")
                                    .on_click(cx.listener(|this, _, _, cx| this.request_close(cx))),
                            ),
                    ),
            )
            .into_any_element()
    }

    fn render_downloads(&self, cx: &mut Context<Self>) -> Option<AnyElement> {
        if self.downloads.is_empty() {
            return None;
        }
        let colors = cx.colors().clone();
        Some(
            h_flex()
                .flex_none()
                .h(u(34.0))
                .px(u(10.0))
                .gap(u(8.0))
                .border_t_1()
                .border_color(colors.border)
                .bg(colors.panel)
                .text_size(u(12.0))
                .child(
                    Icon::new(IconName::Download)
                        .size(13.0)
                        .color(colors.text_dim),
                )
                .children(self.downloads.iter().enumerate().map(|(index, download)| {
                    let (status, color) = match &download.state {
                        DownloadState::Running => ("downloading…", colors.text_dim),
                        DownloadState::Saving => ("choose where to save…", colors.text_dim),
                        DownloadState::Saved(_) => ("saved", colors.green),
                        DownloadState::Failed => ("failed", colors.red),
                    };
                    h_flex()
                        .id(("download", index))
                        .gap(u(6.0))
                        .px(u(8.0))
                        .h(u(24.0))
                        .rounded(u(4.0))
                        .border_1()
                        .border_color(colors.border)
                        .child(div().text_color(colors.text).child(download.name.clone()))
                        .child(div().text_color(color).child(status))
                        .when_some(
                            match &download.state {
                                DownloadState::Saved(path) => Some(path.clone()),
                                _ => None,
                            },
                            |this, path| {
                                this.cursor_pointer()
                                    .on_click(move |_, _, cx| cx.reveal_path(&path))
                            },
                        )
                }))
                .child(div().flex_1())
                .child(
                    IconButton::new("downloads-clear", IconName::X)
                        .icon_size(11.0)
                        .on_click(cx.listener(|this, _, _, cx| {
                            this.downloads.retain(|d| {
                                matches!(d.state, DownloadState::Running | DownloadState::Saving)
                            });
                            cx.notify();
                        })),
                )
                .into_any_element(),
        )
    }
}

fn to_owned(path: &std::path::Path) -> PathBuf {
    path.to_path_buf()
}

/// Moves a finished download out of the staging directory (copying across volumes).
fn move_file(from: &std::path::Path, to: &std::path::Path) -> std::io::Result<()> {
    if std::fs::rename(from, to).is_err() {
        std::fs::copy(from, to)?;
        std::fs::remove_file(from)?;
    }
    if let Some(dir) = from.parent() {
        std::fs::remove_dir(dir).ok();
    }
    Ok(())
}

fn nav_button(
    id: &'static str,
    icon: IconName,
    enabled: bool,
    on_click: impl Fn(&gpui::ClickEvent, &mut Window, &mut App) + 'static,
) -> gpui::Div {
    div().when(!enabled, |this| this.opacity(0.4)).child(
        IconButton::new(id, icon)
            .icon_size(14.0)
            .on_click(move |event, window, cx| {
                if enabled {
                    on_click(event, window, cx)
                }
            }),
    )
}

/// `svc/kube-prom…-grafana:80`: long names shortened in the middle for the address bar.
fn short_label(target: &WebTarget) -> String {
    const MAX: usize = 24;
    let name: Vec<char> = target.name.chars().collect();
    let name = if name.len() > MAX {
        let head: String = name[..MAX / 2 - 1].iter().collect();
        let tail: String = name[name.len() - (MAX / 2 - 1)..].iter().collect();
        format!("{head}…{tail}")
    } else {
        target.name.clone()
    };
    format!("{}/{name}:{}", target.kind.short(), target.port)
}

fn cluster_name(target: &WebTarget, cx: &App) -> String {
    ConnectionManager::try_global(cx)
        .map(|m| m.read(cx).display_name(&target.cluster).to_string())
        .unwrap_or_else(|| target.cluster.to_string())
}

/// Kubyl's web-view data: WebView2's user data folder, WebKitGTK's per-service data, staged
/// downloads. (WKWebView keeps its data stores itself.)
pub fn data_dir() -> PathBuf {
    dirs::data_local_dir()
        .unwrap_or_else(std::env::temp_dir)
        .join("kubyl")
        .join("webview")
}

/// Keystrokes bound to Kubyl actions in the web view's key context (its focus path), that the
/// page must not swallow: ones with Ctrl, Alt, Cmd or Fn, except the page's editing keys.
fn kubyl_shortcuts(window: &Window, cx: &App) -> Vec<Keystroke> {
    let contexts = window.context_stack();
    let keymap = cx.key_bindings();
    let keymap = keymap.borrow();
    let editing = ["a", "c", "v", "x", "z", "y"];
    let mut shortcuts: Vec<Keystroke> = Vec::new();
    for binding in keymap.bindings() {
        if gpui::is_no_action(binding.action()) || gpui::is_unbind(binding.action()) {
            continue;
        }
        if !binding.predicate().is_none_or(|p| p.eval(&contexts)) {
            continue;
        }
        let Some(first) = binding.keystrokes().first() else {
            continue;
        };
        let keystroke = first.inner().clone();
        let m = &keystroke.modifiers;
        let function_key = keystroke.key.len() > 1
            && keystroke.key.starts_with('f')
            && keystroke.key[1..].parse::<u8>().is_ok();
        if !(m.control || m.alt || m.platform || m.function || function_key) {
            continue;
        }
        let is_editing =
            (m.control || m.platform) && !m.alt && editing.contains(&keystroke.key.as_str());
        if is_editing || shortcuts.contains(&keystroke) {
            continue;
        }
        shortcuts.push(keystroke);
    }
    shortcuts
}

impl Focusable for WebViewTab {
    fn focus_handle(&self, _: &App) -> FocusHandle {
        self.content_focus.clone()
    }
}

impl TabView for WebViewTab {
    fn tab_title(&self, cx: &App) -> SharedString {
        self.tab_title(cx).into()
    }

    fn tab_icon(&self, _: &App) -> Option<SharedString> {
        Some(IconName::Globe.path())
    }

    fn tab_dot(&self, cx: &App) -> Option<Hsla> {
        ConnectionManager::try_global(cx).map(|m| m.read(cx).color(&self.target.cluster, cx))
    }

    fn wants_close(&self, _: &App) -> bool {
        self.closing
    }
}

impl Render for WebViewTab {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        self.wake_if_idle(cx);
        self.refresh_shortcuts(window, cx);
        if let Some(embedded) = &self.embedded {
            embedded.set_covered(self.covers_page());
            if embedded.is_shown() {
                self.last_seen = Instant::now();
            }
        }
        let caps = self.caps(cx);
        let colors: Colors = cx.colors().clone();
        let toolbar = self.render_toolbar(&caps, cx);
        let banner = self.render_banner(cx);
        let content = self.render_content(window, cx);
        let downloads = self.render_downloads(cx);
        let menu = self.menu_open.then(|| self.render_menu(cx));
        v_flex()
            .key_context("WebView")
            .track_focus(&self.focus)
            .relative()
            .size_full()
            .bg(colors.background)
            .on_action(
                cx.listener(|this, _: &Copy, _, _| this.with_native(|n| n.edit(EditCommand::Copy))),
            )
            .on_action(
                cx.listener(|this, _: &Cut, _, _| this.with_native(|n| n.edit(EditCommand::Cut))),
            )
            .on_action(
                cx.listener(|this, _: &Paste, _, _| {
                    this.with_native(|n| n.edit(EditCommand::Paste))
                }),
            )
            .on_action(cx.listener(|this, _: &SelectAll, _, _| {
                this.with_native(|n| n.edit(EditCommand::SelectAll))
            }))
            .on_action(
                cx.listener(|this, _: &Undo, _, _| this.with_native(|n| n.edit(EditCommand::Undo))),
            )
            .on_action(
                cx.listener(|this, _: &Redo, _, _| this.with_native(|n| n.edit(EditCommand::Redo))),
            )
            .on_action(cx.listener(|this, _: &ZoomPageIn, _, cx| this.zoom_step(true, cx)))
            .on_action(cx.listener(|this, _: &ZoomPageOut, _, cx| this.zoom_step(false, cx)))
            .on_action(cx.listener(|this, _: &ResetPageZoom, _, cx| this.set_zoom(1.0, cx)))
            .on_action(cx.listener(|this, _: &ReloadPage, window, cx| this.reload(window, cx)))
            .on_action(cx.listener(|this, _: &OpenInBrowser, _, cx| this.open_in_browser(cx)))
            .on_action(cx.listener(|this, _: &CopyUrl, _, cx| this.copy_url(cx)))
            .on_action(cx.listener(|this, _: &ClearSiteData, _, cx| this.clear_site_data(cx)))
            .on_action(cx.listener(|this, _: &DevTools, _, cx| {
                if WebViewSettings::devtools(cx) {
                    this.with_native(|n| n.open_devtools());
                }
            }))
            .map(|this| {
                #[cfg(debug_assertions)]
                let this = this.on_action(cx.listener(|this, _: &crate::DebugInput, _, _| {
                    if let Ok(script) = std::env::var("KUBYL_WEBVIEW_TEST_INPUT") {
                        this.with_native(|n| n.post_test_input(&script));
                    }
                }));
                this
            })
            .on_action(cx.listener(|this, _: &StopForward, _, cx| {
                let target = this.target.clone();
                WebForwards::global(cx).update(cx, |f, cx| f.stop_and_close(&target, cx));
            }))
            .on_action(cx.listener(|this, _: &FocusAddress, window, cx| {
                this.address.update(cx, |input, cx| input.focus(window, cx));
            }))
            .child(toolbar)
            .children(banner)
            .child(
                div()
                    .flex_1()
                    .min_h_0()
                    .flex()
                    .track_focus(&self.content_focus)
                    .child(content),
            )
            .children(downloads)
            .children(menu)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn long_service_names_are_shortened_in_the_middle() {
        let target = |name: &str| WebTarget {
            cluster: kubyl_core::ClusterId::new("c"),
            namespace: "monitoring".into(),
            kind: TargetKind::Service,
            name: name.into(),
            port: 80,
        };
        assert_eq!(short_label(&target("grafana")), "svc/grafana:80");
        assert_eq!(
            short_label(&target("kube-prometheus-stack-grafana")),
            "svc/kube-promet…ack-grafana:80"
        );
    }
}
