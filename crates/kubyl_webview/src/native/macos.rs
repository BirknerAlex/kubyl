//! macOS specifics of the embedded WKWebView:
//!
//! - **Data stores**: one `WKWebsiteDataStore` per service (`dataStoreForIdentifier:`, macOS 14+).
//!   Older systems get a non-persistent store per view, so nothing is ever shared.
//! - **Focus**: clicks into the page don't reach GPUI, so a local event monitor reports them
//!   ([`NativeEvent::Focused`]) and the tab moves GPUI's focus to itself. Kubyl's shortcuts
//!   (⌘K, ⌘W…) then keep working: GPUI's view gets `performKeyEquivalent:` before the page.
//! - **Editing**: Kubyl has no Edit menu, so ⌘C/⌘V/⌘X/⌘A/⌘Z are sent to the page as responder
//!   actions ([`edit`]).
//! - **Certificates**: wry has no hook for server-trust challenges, so one is added to its
//!   navigation delegate class. Loopback HTTPS is accepted only for leaf certificates the user
//!   accepted for this service; anything else is cancelled and reported
//!   ([`NativeEvent::CertificateRejected`]).

use std::cell::RefCell;
use std::collections::HashMap;
use std::ffi::c_void;
use std::ptr::NonNull;
use std::rc::Rc;

use block2::{Block, RcBlock};
use futures::channel::mpsc::UnboundedSender;
use objc2::rc::Retained;
use objc2::runtime::{AnyClass, AnyObject, Imp, Sel};
use objc2::{MainThreadMarker, msg_send, sel};
use objc2_app_kit::{NSEvent, NSEventMask};
use objc2_foundation::{NSOperatingSystemVersion, NSProcessInfo, NSString};
use sha2::{Digest as _, Sha256};
use wry::{WebViewBuilder, WebViewBuilderExtDarwin as _, WebViewExtMacOS as _};

use super::{EditCommand, NativeEvent, NativeOptions, Storage, is_loopback};

/// One embedded view's registration (focus monitor, trust decisions). Dropping it unregisters.
pub struct Attached {
    key: usize,
}

struct Entry {
    webview: Retained<AnyObject>,
    events: UnboundedSender<NativeEvent>,
    accepted: Rc<RefCell<Vec<[u8; 32]>>>,
}

thread_local! {
    static VIEWS: RefCell<HashMap<usize, Entry>> = RefCell::new(HashMap::new());
    static MONITOR: RefCell<Option<Retained<AnyObject>>> = const { RefCell::new(None) };
}

fn os_major_version() -> isize {
    let version: NSOperatingSystemVersion = NSProcessInfo::processInfo().operatingSystemVersion();
    version.majorVersion
}

pub fn configure<'a>(builder: WebViewBuilder<'a>, options: &NativeOptions) -> WebViewBuilder<'a> {
    match &options.storage {
        // Per-service stores need macOS 14; before that, nothing may be shared: private.
        Storage::Isolated { id } if os_major_version() >= 14 => {
            builder.with_data_store_identifier(*id)
        }
        Storage::Isolated { .. } => builder.with_incognito(true),
        Storage::Private => builder,
    }
    .with_allow_link_preview(false)
}

pub fn attach(
    webview: &wry::WebView,
    options: &NativeOptions,
    events: UnboundedSender<NativeEvent>,
) -> anyhow::Result<Attached> {
    // SAFETY: a WryWebView is an NSObject subclass.
    let view: Retained<AnyObject> = unsafe { Retained::cast_unchecked(webview.webview()) };
    let key = Retained::as_ptr(&view) as usize;
    VIEWS.with_borrow_mut(|views| {
        views.insert(
            key,
            Entry {
                webview: view,
                events,
                accepted: options.accepted_certs.clone(),
            },
        )
    });
    install_monitor();
    // The screenshot harness runs behind other windows; WebKit stops painting occluded
    // windows, so its snapshots would be stale. (WebKit SPI, only for the harness.)
    if std::env::var_os("KUBYL_SCREENSHOT").is_some() {
        // SAFETY: a BOOL setter, only called when the view has it.
        unsafe {
            let view = webview.webview();
            let setter = sel!(_setWindowOcclusionDetectionEnabled:);
            let responds: bool = msg_send![&*view, respondsToSelector: setter];
            if responds {
                let _: () = msg_send![&*view, _setWindowOcclusionDetectionEnabled: false];
            }
        }
    }
    // wry's navigation delegate class (its name is mangled per wry version) exists once a
    // view does. WebKit reads which methods a delegate has when it's set: set it again.
    // SAFETY: plain WKWebView / NSObject calls on the view's own navigation delegate.
    unsafe {
        let view = webview.webview();
        let delegate: *mut AnyObject = msg_send![&*view, navigationDelegate];
        if let Some(delegate) = delegate.as_ref()
            && install_trust_hook(delegate.class())
        {
            let _: () = msg_send![&*view, setNavigationDelegate: delegate];
        }
    }
    Ok(Attached { key })
}

impl Drop for Attached {
    fn drop(&mut self) {
        VIEWS.with_borrow_mut(|views| views.remove(&self.key));
    }
}

/// Reports clicks into an embedded page as [`NativeEvent::Focused`].
fn install_monitor() {
    MONITOR.with_borrow_mut(|monitor| {
        if monitor.is_some() {
            return;
        }
        let block = RcBlock::new(|event: NonNull<NSEvent>| -> *mut NSEvent {
            // SAFETY: AppKit passes a valid event and keeps it alive during the call.
            let event_ref = unsafe { event.as_ref() };
            let window = event_ref.window(MainThreadMarker::new().expect("main thread"));
            tracing::debug!(
                has_window = window.is_some(),
                "mouse down seen by the web view monitor"
            );
            if let Some(window) = window {
                let point = event_ref.locationInWindow();
                VIEWS.with_borrow(|views| {
                    for entry in views.values() {
                        if hit(&entry.webview, &window, point) {
                            entry.events.unbounded_send(NativeEvent::Focused).ok();
                        }
                    }
                });
            }
            event.as_ptr()
        });
        let mask = NSEventMask::LeftMouseDown | NSEventMask::RightMouseDown;
        // SAFETY: the block returns the event it was given.
        *monitor = unsafe { NSEvent::addLocalMonitorForEventsMatchingMask_handler(mask, &block) };
    });
}

/// Whether a click at `point` (window coordinates) lands on the visible `webview`.
fn hit(
    webview: &AnyObject,
    window: &objc2_app_kit::NSWindow,
    point: objc2_foundation::NSPoint,
) -> bool {
    // SAFETY: `webview` is a WKWebView (an NSView); these are plain NSView queries.
    unsafe {
        let view_window: *mut AnyObject = msg_send![webview, window];
        if !std::ptr::eq(view_window, window as *const _ as *const AnyObject) {
            return false;
        }
        let hidden: bool = msg_send![webview, isHiddenOrHasHiddenAncestor];
        if hidden {
            return false;
        }
        let local: objc2_foundation::NSPoint =
            msg_send![webview, convertPoint: point, fromView: std::ptr::null::<AnyObject>()];
        let bounds: objc2_foundation::NSRect = msg_send![webview, bounds];
        local.x >= bounds.origin.x
            && local.y >= bounds.origin.y
            && local.x <= bounds.origin.x + bounds.size.width
            && local.y <= bounds.origin.y + bounds.size.height
    }
}

/// Sends an editing action straight to the web view (never up the responder chain: an
/// unhandled action would reach GPUI's app delegate, which is busy dispatching this key).
pub fn edit(webview: &wry::WebView, command: EditCommand) {
    let action = match command {
        EditCommand::Cut => sel!(cut:),
        EditCommand::Copy => sel!(copy:),
        EditCommand::Paste => sel!(paste:),
        EditCommand::SelectAll => sel!(selectAll:),
        EditCommand::Undo => sel!(undo:),
        EditCommand::Redo => sel!(redo:),
    };
    let view = webview.webview();
    // SAFETY: standard NSResponder actions, sent only when the view implements them.
    unsafe {
        let responds: bool = msg_send![&*view, respondsToSelector: action];
        if responds {
            let _: () = msg_send![&*view, performSelector: action, withObject: std::ptr::null::<AnyObject>()];
        } else {
            tracing::debug!(?command, "the web view doesn't handle this editing action");
        }
    }
}

/// Takes a PNG snapshot of the visible page.
pub fn snapshot(webview: &wry::WebView, done: Box<dyn FnOnce(Option<Vec<u8>>)>) {
    let done = RefCell::new(Some(done));
    let block = RcBlock::new(move |image: *mut AnyObject, _error: *mut AnyObject| {
        let Some(done) = done.borrow_mut().take() else {
            return;
        };
        // SAFETY: WebKit passes an NSImage (or nil); TIFF → bitmap rep → PNG is plain AppKit.
        let png = unsafe {
            if image.is_null() {
                None
            } else {
                let tiff: *mut AnyObject = msg_send![image, TIFFRepresentation];
                let rep: *mut AnyObject =
                    msg_send![objc2::class!(NSBitmapImageRep), imageRepWithData: tiff];
                if rep.is_null() {
                    None
                } else {
                    let properties: *mut AnyObject =
                        msg_send![objc2::class!(NSDictionary), dictionary];
                    // NSBitmapImageFileTypePNG
                    let data: *mut AnyObject =
                        msg_send![rep, representationUsingType: 4usize, properties: properties];
                    if data.is_null() {
                        None
                    } else {
                        let length: usize = msg_send![data, length];
                        let bytes: *const u8 = msg_send![data, bytes];
                        Some(std::slice::from_raw_parts(bytes, length).to_vec())
                    }
                }
            }
        };
        done(png);
    });
    let view = webview.webview();
    // SAFETY: `takeSnapshotWithConfiguration:completionHandler:` with a nil configuration
    // snapshots the visible bounds; the block matches `(NSImage *, NSError *)`.
    unsafe {
        let _: () = msg_send![
            &*view,
            takeSnapshotWithConfiguration: std::ptr::null::<AnyObject>(),
            completionHandler: &*block
        ];
    }
}

// --- Server trust ---------------------------------------------------------------------------

/// `NSURLSessionAuthChallengeDisposition`.
const USE_CREDENTIAL: isize = 0;
const PERFORM_DEFAULT_HANDLING: isize = 1;
const CANCEL_CHALLENGE: isize = 2;

type TrustHandler = Block<dyn Fn(isize, *mut AnyObject)>;

#[allow(non_camel_case_types)]
type SecTrustRef = *const c_void;
#[allow(non_camel_case_types)]
type CFTypeRef = *const c_void;

#[link(name = "Security", kind = "framework")]
unsafe extern "C" {
    fn SecTrustGetCertificateCount(trust: SecTrustRef) -> isize;
    fn SecTrustGetCertificateAtIndex(trust: SecTrustRef, index: isize) -> CFTypeRef;
    fn SecCertificateCopyData(certificate: CFTypeRef) -> CFTypeRef;
}

#[link(name = "CoreFoundation", kind = "framework")]
unsafe extern "C" {
    fn CFDataGetLength(data: CFTypeRef) -> isize;
    fn CFDataGetBytePtr(data: CFTypeRef) -> *const u8;
    fn CFRelease(value: CFTypeRef);
}

/// The certificate chain of a trust object, leaf first, as DER.
///
/// # Safety
/// `trust` must be a valid `SecTrustRef`.
unsafe fn chain(trust: SecTrustRef) -> Vec<Vec<u8>> {
    let mut chain = Vec::new();
    // SAFETY: the caller passes a valid trust; certificates are borrowed from it, their data
    // is copied and released.
    unsafe {
        for index in 0..SecTrustGetCertificateCount(trust) {
            let certificate = SecTrustGetCertificateAtIndex(trust, index);
            if certificate.is_null() {
                continue;
            }
            let data = SecCertificateCopyData(certificate);
            if data.is_null() {
                continue;
            }
            let bytes = std::slice::from_raw_parts(
                CFDataGetBytePtr(data),
                CFDataGetLength(data).max(0) as usize,
            );
            chain.push(bytes.to_vec());
            CFRelease(data);
        }
    }
    chain
}

/// Adds the server-trust method to wry's navigation delegate class. Returns whether it was
/// added now (the delegate must be set again) or false when it's there already or failed.
fn install_trust_hook(class: &AnyClass) -> bool {
    let selector = sel!(webView:didReceiveAuthenticationChallenge:completionHandler:);
    if class.instance_method(selector).is_some() {
        return false;
    }
    // SAFETY: the implementation matches the WKNavigationDelegate method's signature
    // (`v@:@@@?`): self, _cmd, webView, challenge, completion handler.
    unsafe {
        let imp: Imp = std::mem::transmute::<
            unsafe extern "C-unwind" fn(
                *mut AnyObject,
                Sel,
                *mut AnyObject,
                *mut AnyObject,
                *mut TrustHandler,
            ),
            Imp,
        >(did_receive_challenge);
        let added = objc2::ffi::class_addMethod(
            class as *const AnyClass as *mut AnyClass,
            selector,
            imp,
            c"v@:@@@?".as_ptr(),
        );
        if !added.as_bool() {
            tracing::warn!("couldn't add the server trust hook to wry's navigation delegate");
        }
        added.as_bool()
    }
}

unsafe extern "C-unwind" fn did_receive_challenge(
    _this: *mut AnyObject,
    _cmd: Sel,
    webview: *mut AnyObject,
    challenge: *mut AnyObject,
    handler: *mut TrustHandler,
) {
    // SAFETY: WebKit passes a live challenge and completion handler; everything below is
    // documented NSURLAuthenticationChallenge / NSURLProtectionSpace API.
    unsafe {
        let Some(handler) = handler.as_ref() else {
            return;
        };
        let space: *mut AnyObject = msg_send![challenge, protectionSpace];
        let method: *mut NSString = msg_send![space, authenticationMethod];
        let is_server_trust = method
            .as_ref()
            .is_some_and(|m| m.to_string() == "NSURLAuthenticationMethodServerTrust");
        let host: *mut NSString = msg_send![space, host];
        let host = host.as_ref().map(|h| h.to_string()).unwrap_or_default();
        if !is_server_trust || !is_loopback(&host) {
            handler.call((PERFORM_DEFAULT_HANDLING, std::ptr::null_mut()));
            return;
        }
        let trust: SecTrustRef = msg_send![space, serverTrust];
        if trust.is_null() {
            handler.call((PERFORM_DEFAULT_HANDLING, std::ptr::null_mut()));
            return;
        }
        let chain = chain(trust);
        let fingerprint: Option<[u8; 32]> = chain.first().map(|leaf| Sha256::digest(leaf).into());
        let decision = VIEWS.with_borrow(|views| {
            let entry = views.get(&(webview as usize))?;
            let accepted = fingerprint.is_some_and(|f| entry.accepted.borrow().contains(&f));
            if !accepted {
                entry
                    .events
                    .unbounded_send(NativeEvent::CertificateRejected {
                        chain: chain.clone(),
                    })
                    .ok();
            }
            Some(accepted)
        });
        if decision == Some(true) {
            let credential: *mut AnyObject =
                msg_send![objc2::class!(NSURLCredential), credentialForTrust: trust];
            handler.call((USE_CREDENTIAL, credential));
        } else {
            handler.call((CANCEL_CHALLENGE, std::ptr::null_mut()));
        }
    }
}

/// Test driver: posts real AppKit events (clicks at view-relative fractions, typed text,
/// shortcuts) so the key routing between GPUI and the page can be checked end to end.
/// `script`: `click:0.3:0.2;type:hello;key:cmd-k;eval:document.title=…` (steps run 300 ms
/// apart; `eval` runs JavaScript in the page, e.g. to report a field through the title).
#[cfg(debug_assertions)]
pub fn post_test_input(webview: &wry::WebView, script: &str) {
    let view = webview.webview();
    let steps: Vec<String> = script.split(';').map(str::to_string).collect();
    let view: Retained<AnyObject> = unsafe { Retained::cast_unchecked(view) };
    for (index, step) in steps.into_iter().enumerate() {
        let view = view.clone();
        let delay = 0.3 * index as f64;
        let block = RcBlock::new(move || unsafe { post_step(&view, &step) });
        unsafe {
            let queue = dispatch_get_main_queue();
            let when = dispatch_time(0, (delay * 1e9) as i64);
            dispatch_after(when, queue, &*block as *const _ as *mut c_void);
        }
    }
}

#[cfg(debug_assertions)]
unsafe extern "C" {
    static _dispatch_main_q: c_void;
    fn dispatch_time(when: u64, delta: i64) -> u64;
    fn dispatch_after(when: u64, queue: *const c_void, block: *mut c_void);
}

#[cfg(debug_assertions)]
unsafe fn dispatch_get_main_queue() -> *const c_void {
    &raw const _dispatch_main_q
}

#[cfg(debug_assertions)]
unsafe fn post_step(view: &AnyObject, step: &str) {
    unsafe {
        let window: *mut AnyObject = msg_send![view, window];
        if window.is_null() {
            return;
        }
        let number: isize = msg_send![window, windowNumber];
        let app: *mut AnyObject = msg_send![objc2::class!(NSApplication), sharedApplication];
        let _: () = msg_send![app, activateIgnoringOtherApps: true];
        let _: () = msg_send![window, makeKeyAndOrderFront: std::ptr::null::<AnyObject>()];
        let responder: *mut AnyObject = msg_send![window, firstResponder];
        let class_name = if responder.is_null() {
            "nil".to_string()
        } else {
            let class: *const AnyClass = msg_send![responder, class];
            (*class).name().to_string_lossy().into_owned()
        };
        let key: bool = msg_send![window, isKeyWindow];
        tracing::info!(
            step,
            first_responder = class_name,
            key_window = key,
            "test input step"
        );
        let post = |event: *mut AnyObject| {
            let _: () = msg_send![app, postEvent: event, atStart: false];
        };
        if let Some(script) = step.strip_prefix("eval:") {
            let script = NSString::from_str(script);
            let _: () = msg_send![
                view,
                evaluateJavaScript: &*script,
                completionHandler: std::ptr::null::<AnyObject>()
            ];
            return;
        }
        let mut parts = step.split(':');
        match parts.next() {
            Some("click") => {
                let fx: f64 = parts.next().and_then(|v| v.parse().ok()).unwrap_or(0.5);
                let fy: f64 = parts.next().and_then(|v| v.parse().ok()).unwrap_or(0.5);
                let bounds: objc2_foundation::NSRect = msg_send![view, bounds];
                // Flipped views have y down; convert from the view to window coordinates.
                let local =
                    objc2_foundation::NSPoint::new(bounds.size.width * fx, bounds.size.height * fy);
                let point: objc2_foundation::NSPoint =
                    msg_send![view, convertPoint: local, toView: std::ptr::null::<AnyObject>()];
                for kind in [1usize, 2usize] {
                    let event: *mut AnyObject = msg_send![
                        objc2::class!(NSEvent),
                        mouseEventWithType: kind,
                        location: point,
                        modifierFlags: 0usize,
                        timestamp: 0.0f64,
                        windowNumber: number,
                        context: std::ptr::null::<AnyObject>(),
                        eventNumber: 0isize,
                        clickCount: 1isize,
                        pressure: 1.0f32
                    ];
                    post(event);
                }
            }
            Some("type") => {
                for c in parts.next().unwrap_or_default().chars() {
                    post_key(&post, number, &c.to_string(), 0, key_code(c));
                }
            }
            Some("key") => {
                let spec = parts.next().unwrap_or_default();
                let mut flags = 0usize;
                let mut key = "";
                for part in spec.split('-') {
                    match part {
                        "cmd" => flags |= 1 << 20,
                        "ctrl" => flags |= 1 << 18,
                        "shift" => flags |= 1 << 17,
                        "alt" => flags |= 1 << 19,
                        other => key = other,
                    }
                }
                let (chars, code) = match key {
                    "tab" => ("\t".to_string(), 0x30),
                    "enter" => ("\r".to_string(), 0x24),
                    other => (
                        other.to_string(),
                        other.chars().next().map(key_code).unwrap_or(0),
                    ),
                };
                post_key(&post, number, &chars, flags, code);
            }
            _ => {}
        }
    }
}

/// Delivers a key down/up pair the way NSApplication routes it to the key window: key
/// equivalents through the window's view hierarchy first, then the first responder.
#[cfg(debug_assertions)]
unsafe fn post_key(
    _post: &dyn Fn(*mut AnyObject),
    window: isize,
    chars: &str,
    flags: usize,
    code: u16,
) {
    unsafe {
        let text = NSString::from_str(chars);
        let app: *mut AnyObject = msg_send![objc2::class!(NSApplication), sharedApplication];
        let target: *mut AnyObject = msg_send![app, windowWithWindowNumber: window];
        if target.is_null() {
            return;
        }
        for kind in [10usize, 11usize] {
            let event: *mut AnyObject = msg_send![
                objc2::class!(NSEvent),
                keyEventWithType: kind,
                location: objc2_foundation::NSPoint::new(0.0, 0.0),
                modifierFlags: flags,
                timestamp: 0.0f64,
                windowNumber: window,
                context: std::ptr::null::<AnyObject>(),
                characters: &*text,
                charactersIgnoringModifiers: &*text,
                isARepeat: false,
                keyCode: code
            ];
            let modified = flags & ((1 << 20) | (1 << 18)) != 0;
            if kind == 10 && modified {
                let handled: bool = msg_send![target, performKeyEquivalent: event];
                tracing::info!(chars, handled, "test key equivalent");
                if handled {
                    continue;
                }
            }
            let _: () = msg_send![target, sendEvent: event];
        }
    }
}

#[cfg(debug_assertions)]
fn key_code(c: char) -> u16 {
    match c.to_ascii_lowercase() {
        'a' => 0x00,
        's' => 0x01,
        'd' => 0x02,
        'f' => 0x03,
        'h' => 0x04,
        'g' => 0x05,
        'z' => 0x06,
        'x' => 0x07,
        'c' => 0x08,
        'v' => 0x09,
        'b' => 0x0B,
        'q' => 0x0C,
        'w' => 0x0D,
        'e' => 0x0E,
        'r' => 0x0F,
        'y' => 0x10,
        't' => 0x11,
        'o' => 0x1F,
        'u' => 0x20,
        'i' => 0x22,
        'p' => 0x23,
        'l' => 0x25,
        'j' => 0x26,
        'k' => 0x28,
        'n' => 0x2D,
        'm' => 0x2E,
        ' ' => 0x31,
        '[' => 0x21,
        ']' => 0x1E,
        _ => 0x00,
    }
}
