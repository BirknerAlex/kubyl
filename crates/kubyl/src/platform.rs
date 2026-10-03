//! Platform integration that bundling can't cover during development.

use std::sync::Arc;

use gpui::App;

pub fn init(_cx: &mut App) {
    #[cfg(target_os = "macos")]
    macos::set_dock_icon();
}

/// Linux: runs Kubyl through XWayland when a Wayland session offers it, so web views can be
/// child windows of the main window. A Wayland client can't embed WebKitGTK's surface (it
/// lives on GTK's own connection), so on plain Wayland every page needs a window of its own.
/// `KUBYL_WAYLAND=1` keeps the native Wayland window. Call first in `main`, before threads
/// start. Returns whether it switched to X11.
#[cfg(target_os = "linux")]
pub fn prefer_xwayland() -> bool {
    use std::os::unix::net::UnixStream;

    if std::env::var_os("KUBYL_WAYLAND").is_some_and(|v| !v.is_empty()) {
        return false;
    }
    if std::env::var_os("WAYLAND_DISPLAY").is_none_or(|v| v.is_empty()) {
        return false;
    }
    let reachable = std::env::var("DISPLAY")
        .ok()
        .and_then(|display| x11_socket(&display))
        .is_some_and(|socket| UnixStream::connect(socket).is_ok());
    if !reachable {
        return false;
    }
    // GPUI picks X11 when there is no Wayland display.
    // SAFETY: nothing else runs yet, so no thread reads the environment concurrently.
    unsafe { std::env::remove_var("WAYLAND_DISPLAY") };
    true
}

/// The Unix socket of a local X display (`:0`, `:1.0`); `None` for remote displays.
#[cfg(target_os = "linux")]
fn x11_socket(display: &str) -> Option<std::path::PathBuf> {
    let number: u32 = display.strip_prefix(':')?.split('.').next()?.parse().ok()?;
    Some(format!("/tmp/.X11-unix/X{number}").into())
}

#[cfg(all(test, target_os = "linux"))]
mod tests {
    use super::*;

    #[test]
    fn local_displays_map_to_sockets() {
        assert_eq!(x11_socket(":0"), Some("/tmp/.X11-unix/X0".into()));
        assert_eq!(x11_socket(":1.0"), Some("/tmp/.X11-unix/X1".into()));
        assert_eq!(x11_socket("host:0"), None);
        assert_eq!(x11_socket(""), None);
    }
}

/// Raises the open-file soft limit to what the OS allows. Apps launched from the Finder get
/// 256, which Kubyl outgrows with a few clusters (HTTP/1.1: a socket per watch) and web views;
/// WebKit aborts the process when it can't open what it needs. Call first in `main`, before
/// threads start. Returns the (old, new) soft limit when it changed, `None` when it was
/// already high enough.
#[cfg(unix)]
pub fn raise_fd_limit() -> std::io::Result<Option<(u64, u64)>> {
    let mut limit = libc::rlimit {
        rlim_cur: 0,
        rlim_max: 0,
    };
    // SAFETY: `limit` is a valid, writable rlimit.
    if unsafe { libc::getrlimit(libc::RLIMIT_NOFILE, &mut limit) } != 0 {
        return Err(std::io::Error::last_os_error());
    }
    let old = limit.rlim_cur;
    // macOS rejects values above `OPEN_MAX` (10240), even with an unlimited hard limit.
    let wanted = if cfg!(target_os = "macos") {
        limit.rlim_max.min(10240)
    } else {
        limit.rlim_max
    };
    if wanted <= old {
        return Ok(None);
    }
    limit.rlim_cur = wanted;
    // SAFETY: `limit` is a valid rlimit; raising the soft limit up to the hard one is allowed.
    if unsafe { libc::setrlimit(libc::RLIMIT_NOFILE, &limit) } != 0 {
        return Err(std::io::Error::last_os_error());
    }
    Ok(Some((old, wanted)))
}

#[cfg(not(unix))]
pub fn raise_fd_limit() -> std::io::Result<Option<(u64, u64)>> {
    Ok(None)
}

/// The window icon, used by X11 (Wayland and the other platforms take it from the app bundle
/// or desktop file).
pub fn window_icon() -> Option<Arc<image::RgbaImage>> {
    if !cfg!(any(target_os = "linux", target_os = "freebsd")) {
        return None;
    }
    let bytes = include_bytes!("../../../assets/logo/png/app-icon-128.png");
    image::load_from_memory_with_format(bytes, image::ImageFormat::Png)
        .map(|icon| Arc::new(icon.into_rgba8()))
        .inspect_err(|err| tracing::warn!("failed to decode the window icon: {err}"))
        .ok()
}

#[cfg(target_os = "macos")]
mod macos {
    use objc2::{AllocAnyThread as _, MainThreadMarker};
    use objc2_app_kit::{NSApplication, NSImage};
    use objc2_foundation::NSData;

    /// `cargo run` starts an unbundled binary, which gets a generic Dock icon. Setting it at
    /// runtime shows the Kubyl icon; bundled builds get it from `Info.plist` as well.
    pub fn set_dock_icon() {
        let Some(mtm) = MainThreadMarker::new() else {
            return;
        };
        let bytes = include_bytes!("../../../assets/logo/png/app-icon-512.png");
        let data = NSData::with_bytes(bytes);
        let Some(image) = NSImage::initWithData(NSImage::alloc(), &data) else {
            return;
        };
        let app = NSApplication::sharedApplication(mtm);
        // SAFETY: called on the main thread with a valid image.
        unsafe { app.setApplicationIconImage(Some(&image)) };
    }
}
