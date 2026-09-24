//! Platform integration that bundling can't cover during development.

use std::sync::Arc;

use gpui::App;

pub fn init(_cx: &mut App) {
    #[cfg(target_os = "macos")]
    macos::set_dock_icon();
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
