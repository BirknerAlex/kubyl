//! Windows specifics of the embedded WebView2:
//!
//! - **Data stores**: one user data folder for Kubyl, one WebView2 profile per service.
//! - **Focus**: the controller's `GotFocus` reports clicks into the page
//!   ([`NativeEvent::Focused`]).
//! - **Shortcuts**: the page's child window gets the keyboard, so GPUI never sees Kubyl's
//!   shortcuts. `AcceleratorKeyPressed` catches the ones Kubyl binds (Ctrl-K, Ctrl-W…), marks
//!   them handled and reports them ([`NativeEvent::Shortcut`]). Editing keys stay with the page.
//! - **Certificates**: `ServerCertificateErrorDetected` allows loopback HTTPS only for leaf
//!   certificates the user accepted for this service; anything else is cancelled and reported.

use futures::channel::mpsc::UnboundedSender;
use gpui::{Keystroke, Modifiers};
use sha2::{Digest as _, Sha256};
use webview2_com::Microsoft::Web::WebView2::Win32::{
    COREWEBVIEW2_KEY_EVENT_KIND, COREWEBVIEW2_KEY_EVENT_KIND_KEY_DOWN,
    COREWEBVIEW2_KEY_EVENT_KIND_SYSTEM_KEY_DOWN,
    COREWEBVIEW2_SERVER_CERTIFICATE_ERROR_ACTION_ALWAYS_ALLOW,
    COREWEBVIEW2_SERVER_CERTIFICATE_ERROR_ACTION_CANCEL, ICoreWebView2_14,
};
use webview2_com::{
    AcceleratorKeyPressedEventHandler, FocusChangedEventHandler, ProcessFailedEventHandler,
    ServerCertificateErrorDetectedEventHandler, take_pwstr,
};
use windows::Win32::UI::Input::KeyboardAndMouse::GetKeyState;
use windows::core::{Interface as _, PWSTR};
use wry::{WebViewBuilder, WebViewBuilderExtWindows as _, WebViewExtWindows as _};

use super::{NativeEvent, NativeOptions, Storage, is_loopback_url, is_shortcut, pem_to_der};

/// Keeps the event handlers registered for the view's lifetime.
pub struct Attached;

pub fn configure<'a>(builder: WebViewBuilder<'a>, options: &NativeOptions) -> WebViewBuilder<'a> {
    let builder = builder.with_default_context_menus(true);
    match &options.storage {
        Storage::Isolated { id } => builder.with_profile_name(super::storage_name(id)),
        Storage::Private => builder,
    }
}

pub fn attach(
    webview: &wry::WebView,
    options: &NativeOptions,
    events: UnboundedSender<NativeEvent>,
) -> anyhow::Result<Attached> {
    let controller = webview.controller();
    let core = webview.webview();
    let mut token = 0i64;

    let shortcuts = options.shortcuts.clone();
    let key_events = events.clone();
    // SAFETY: registering COM event handlers on a live controller/webview (UI thread).
    unsafe {
        controller.add_AcceleratorKeyPressed(
            &AcceleratorKeyPressedEventHandler::create(Box::new(move |_, args| {
                let Some(args) = args else {
                    return Ok(());
                };
                let mut kind = COREWEBVIEW2_KEY_EVENT_KIND::default();
                args.KeyEventKind(&mut kind)?;
                if kind != COREWEBVIEW2_KEY_EVENT_KIND_KEY_DOWN
                    && kind != COREWEBVIEW2_KEY_EVENT_KIND_SYSTEM_KEY_DOWN
                {
                    return Ok(());
                }
                let mut virtual_key = 0u32;
                args.VirtualKey(&mut virtual_key)?;
                if let Some(keystroke) = keystroke(virtual_key)
                    && is_shortcut(&shortcuts.borrow(), &keystroke)
                {
                    args.SetHandled(true)?;
                    key_events
                        .unbounded_send(NativeEvent::Shortcut(keystroke))
                        .ok();
                }
                Ok(())
            })),
            &mut token,
        )?;

        let focus_events = events.clone();
        controller.add_GotFocus(
            &FocusChangedEventHandler::create(Box::new(move |_, _| {
                focus_events.unbounded_send(NativeEvent::Focused).ok();
                Ok(())
            })),
            &mut token,
        )?;

        let crash_events = events.clone();
        core.add_ProcessFailed(
            &ProcessFailedEventHandler::create(Box::new(move |_, _| {
                crash_events.unbounded_send(NativeEvent::Crashed).ok();
                Ok(())
            })),
            &mut token,
        )?;

        // Needs WebView2 runtime 1.0.1245 or later; older runtimes show their own error page.
        if let Ok(core) = core.cast::<ICoreWebView2_14>() {
            let accepted = options.accepted_certs.clone();
            let cert_events = events;
            core.add_ServerCertificateErrorDetected(
                &ServerCertificateErrorDetectedEventHandler::create(Box::new(move |_, args| {
                    let Some(args) = args else {
                        return Ok(());
                    };
                    let mut uri = PWSTR::null();
                    args.RequestUri(&mut uri)?;
                    if !is_loopback_url(&take_pwstr(uri)) {
                        return Ok(());
                    }
                    let certificate = args.ServerCertificate()?;
                    let mut pem = PWSTR::null();
                    certificate.ToPemEncoding(&mut pem)?;
                    let der = pem_to_der(&take_pwstr(pem)).unwrap_or_default();
                    let fingerprint: [u8; 32] = Sha256::digest(&der).into();
                    if !der.is_empty() && accepted.borrow().contains(&fingerprint) {
                        args.SetAction(COREWEBVIEW2_SERVER_CERTIFICATE_ERROR_ACTION_ALWAYS_ALLOW)?;
                    } else {
                        args.SetAction(COREWEBVIEW2_SERVER_CERTIFICATE_ERROR_ACTION_CANCEL)?;
                        cert_events
                            .unbounded_send(NativeEvent::CertificateRejected { chain: vec![der] })
                            .ok();
                    }
                    Ok(())
                })),
                &mut token,
            )?;
        }
    }
    Ok(Attached)
}

/// The GPUI keystroke for a virtual key with the current modifier state.
fn keystroke(virtual_key: u32) -> Option<Keystroke> {
    let down = |key: i32| {
        // SAFETY: GetKeyState only reads the calling thread's keyboard state.
        unsafe { GetKeyState(key) < 0 }
    };
    let modifiers = Modifiers {
        control: down(0x11),
        alt: down(0x12),
        shift: down(0x10),
        platform: down(0x5B) || down(0x5C),
        function: false,
    };
    Some(Keystroke {
        modifiers,
        key: key_name(virtual_key)?,
        key_char: None,
    })
}

/// GPUI's name for a virtual key (US layout for punctuation).
fn key_name(virtual_key: u32) -> Option<String> {
    let name = match virtual_key {
        0x30..=0x39 | 0x41..=0x5A => {
            return char::from_u32(virtual_key).map(|c| c.to_ascii_lowercase().to_string());
        }
        0x70..=0x7B => return Some(format!("f{}", virtual_key - 0x6F)),
        0x08 => "backspace",
        0x09 => "tab",
        0x0D => "enter",
        0x1B => "escape",
        0x20 => "space",
        0x21 => "pageup",
        0x22 => "pagedown",
        0x23 => "end",
        0x24 => "home",
        0x25 => "left",
        0x26 => "up",
        0x27 => "right",
        0x28 => "down",
        0x2E => "delete",
        0xBA => ";",
        0xBB => "=",
        0xBC => ",",
        0xBD => "-",
        0xBE => ".",
        0xBF => "/",
        0xC0 => "`",
        0xDB => "[",
        0xDC => "\\",
        0xDD => "]",
        0xDE => "'",
        _ => return None,
    };
    Some(name.to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn virtual_keys_map_to_gpui_names() {
        assert_eq!(key_name(0x4B).as_deref(), Some("k"));
        assert_eq!(key_name(0x31).as_deref(), Some("1"));
        assert_eq!(key_name(0x09).as_deref(), Some("tab"));
        assert_eq!(key_name(0xDD).as_deref(), Some("]"));
        assert_eq!(key_name(0x74).as_deref(), Some("f5"));
        assert_eq!(key_name(0xFF), None);
    }
}
