//! Linux specifics: WebKitGTK through wry.
//!
//! - **GTK**: initialized on the first web view (not at startup) and pumped by a GPUI task
//!   ([`pump`]) while any view exists.
//! - **X11**: the view is a child window of GPUI's X11 window (wry's `build_as_child`).
//! - **Wayland**: a Wayland client can't put another client's surface inside its window, so
//!   the page opens in a GTK window of its own; the tab drives it and closing either closes
//!   both ([`NativeEvent::WindowClosed`]).
//! - **Data**: a WebKit context (data directory) per service.
//! - **Shortcuts, focus, certificates**: signals on the WebKit view (`key-press-event`,
//!   `focus-in-event`, `load-failed-with-tls-errors`).

use std::cell::Cell;

use futures::channel::mpsc::UnboundedSender;
use gpui::{Keystroke, Modifiers};
use gtk::prelude::*;
use sha2::{Digest as _, Sha256};
use webkit2gtk::{
    TLSErrorsPolicy, WebContextExt as _, WebProcessTerminationReason, WebViewExt as _,
    WebsiteDataManagerExt as _,
};
use wry::{WebViewBuilder, WebViewBuilderExtUnix as _, WebViewExtUnix as _};

use super::{
    NativeEvent, NativeOptions, ParentWindow, Placement, is_loopback_url, is_shortcut, pem_to_der,
};

thread_local! {
    static GTK_READY: Cell<bool> = const { Cell::new(false) };
}

/// Initializes GTK on the UI thread (once, on the first web view).
pub fn ensure_gtk() -> anyhow::Result<()> {
    if GTK_READY.get() {
        return Ok(());
    }
    gtk::init().map_err(|err| anyhow::anyhow!("GTK couldn't start: {err}"))?;
    GTK_READY.set(true);
    Ok(())
}

/// Runs pending GTK events (WebKitGTK does its work in GTK's main loop).
pub fn pump() {
    if !GTK_READY.get() {
        return;
    }
    while gtk::events_pending() {
        gtk::main_iteration_do(false);
    }
}

/// The page's own window on Wayland.
pub struct Attached {
    window: Option<gtk::Window>,
}

impl Attached {
    pub fn present(&self) {
        if let Some(window) = &self.window {
            window.present();
        }
    }

    pub fn set_title(&self, title: &str) {
        if let Some(window) = &self.window {
            window.set_title(title);
        }
    }
}

impl Drop for Attached {
    fn drop(&mut self) {
        if let Some(window) = self.window.take() {
            // SAFETY: the window is ours and nothing uses it after this.
            unsafe { window.destroy() };
        }
    }
}

pub fn build(
    builder: WebViewBuilder<'_>,
    parent: &ParentWindow,
    options: &NativeOptions,
    events: &UnboundedSender<NativeEvent>,
) -> anyhow::Result<(wry::WebView, Option<gtk::Window>)> {
    ensure_gtk()?;
    match parent.placement() {
        Placement::Embedded => Ok((builder.build_as_child(parent)?, None)),
        Placement::Window => {
            let window = gtk::Window::new(gtk::WindowType::Toplevel);
            window.set_title(&options.title);
            window.set_default_size(1200, 800);
            let container = gtk::Box::new(gtk::Orientation::Vertical, 0);
            window.add(&container);
            let webview = builder.build_gtk(&container)?;
            let closed = events.clone();
            window.connect_delete_event(move |_, _| {
                closed.unbounded_send(NativeEvent::WindowClosed).ok();
                gtk::glib::Propagation::Proceed
            });
            window.show_all();
            Ok((webview, Some(window)))
        }
    }
}

pub fn attach(
    webview: &wry::WebView,
    window: Option<gtk::Window>,
    options: &NativeOptions,
    events: UnboundedSender<NativeEvent>,
) -> anyhow::Result<Attached> {
    let view = webview.webview();

    let shortcuts = options.shortcuts.clone();
    let key_events = events.clone();
    view.connect_key_press_event(move |_, event| match keystroke(event) {
        Some(keystroke) if is_shortcut(&shortcuts.borrow(), &keystroke) => {
            key_events
                .unbounded_send(NativeEvent::Shortcut(keystroke))
                .ok();
            gtk::glib::Propagation::Stop
        }
        _ => gtk::glib::Propagation::Proceed,
    });

    let focus_events = events.clone();
    view.connect_focus_in_event(move |_, _| {
        focus_events.unbounded_send(NativeEvent::Focused).ok();
        gtk::glib::Propagation::Proceed
    });

    let crash_events = events.clone();
    view.connect_web_process_terminated(move |_, reason| {
        if reason != WebProcessTerminationReason::TerminatedByApi {
            crash_events.unbounded_send(NativeEvent::Crashed).ok();
        }
    });

    // Report TLS errors instead of failing silently; loopback certificates the user accepted
    // are allowed for the host and the page reloads.
    if let Some(manager) = view.context().and_then(|c| c.website_data_manager()) {
        manager.set_tls_errors_policy(TLSErrorsPolicy::Fail);
    }
    let accepted = options.accepted_certs.clone();
    let cert_events = events;
    view.connect_load_failed_with_tls_errors(move |view, uri, certificate, _| {
        if !is_loopback_url(uri) {
            return false;
        }
        let der = certificate
            .certificate_pem()
            .and_then(|pem| pem_to_der(&pem))
            .unwrap_or_default();
        let fingerprint: [u8; 32] = Sha256::digest(&der).into();
        if !der.is_empty() && accepted.borrow().contains(&fingerprint) {
            let host = url::Url::parse(uri)
                .ok()
                .and_then(|u| u.host_str().map(String::from));
            if let (Some(context), Some(host)) = (view.context(), host) {
                context.allow_tls_certificate_for_host(certificate, &host);
                view.load_uri(uri);
            }
        } else {
            cert_events
                .unbounded_send(NativeEvent::CertificateRejected { chain: vec![der] })
                .ok();
        }
        true
    });

    Ok(Attached { window })
}

/// The GPUI keystroke of a GDK key event (unshifted key names, like GPUI's bindings).
fn keystroke(event: &gtk::gdk::EventKey) -> Option<Keystroke> {
    let state = event.state();
    let modifiers = Modifiers {
        control: state.contains(gtk::gdk::ModifierType::CONTROL_MASK),
        alt: state.contains(gtk::gdk::ModifierType::MOD1_MASK),
        shift: state.contains(gtk::gdk::ModifierType::SHIFT_MASK),
        platform: state.contains(gtk::gdk::ModifierType::SUPER_MASK),
        function: false,
    };
    let name = event.keyval().name()?;
    Some(Keystroke {
        modifiers,
        key: key_name(&name)?,
        key_char: None,
    })
}

/// GPUI's name for a GDK keysym name.
fn key_name(keysym: &str) -> Option<String> {
    let name = match keysym {
        "Tab" | "ISO_Left_Tab" => "tab",
        "Return" | "KP_Enter" => "enter",
        "Escape" => "escape",
        "BackSpace" => "backspace",
        "Delete" => "delete",
        "space" => "space",
        "Left" => "left",
        "Right" => "right",
        "Up" => "up",
        "Down" => "down",
        "Home" => "home",
        "End" => "end",
        "Page_Up" => "pageup",
        "Page_Down" => "pagedown",
        "bracketleft" | "braceleft" => "[",
        "bracketright" | "braceright" => "]",
        "backslash" | "bar" => "\\",
        "minus" | "underscore" => "-",
        "equal" | "plus" => "=",
        "comma" | "less" => ",",
        "period" | "greater" => ".",
        "slash" | "question" => "/",
        "semicolon" | "colon" => ";",
        "apostrophe" | "quotedbl" => "'",
        "grave" | "asciitilde" => "`",
        "exclam" => "1",
        "at" => "2",
        "numbersign" => "3",
        "dollar" => "4",
        "percent" => "5",
        "asciicircum" => "6",
        "ampersand" => "7",
        "asterisk" => "8",
        "parenleft" => "9",
        "parenright" => "0",
        other => {
            if let Some(number) = other.strip_prefix('F')
                && number.parse::<u8>().is_ok()
            {
                return Some(format!("f{number}"));
            }
            let mut chars = other.chars();
            return match (chars.next(), chars.next()) {
                (Some(c), None) if c.is_ascii_alphanumeric() => {
                    Some(c.to_ascii_lowercase().to_string())
                }
                _ => None,
            };
        }
    };
    Some(name.to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn gdk_keysyms_map_to_gpui_names() {
        assert_eq!(key_name("k").as_deref(), Some("k"));
        assert_eq!(key_name("K").as_deref(), Some("k"));
        assert_eq!(key_name("ISO_Left_Tab").as_deref(), Some("tab"));
        assert_eq!(key_name("braceright").as_deref(), Some("]"));
        assert_eq!(key_name("F5").as_deref(), Some("f5"));
        assert_eq!(key_name("dead_acute"), None);
    }
}
