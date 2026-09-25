//! Keeps native web views in step with GPUI's frames.
//!
//! A platform web view is a native child view: it's drawn by the OS on top of the GPUI surface
//! and knows nothing about tabs, docks or dialogs. So:
//!
//! - [`WebContent`] (the element standing in for the page) places the view at its bounds every
//!   time it's painted and marks it as painted.
//! - At the end of each frame of a window, [`end_frame`] shows the views that were painted and
//!   hides the others (inactive tabs, zoomed-away panes, closed docks). It also hides them
//!   while something is drawn over them (dialogs, the command palette, sheets, toasts), since
//!   GPUI can't draw on top of a native view.
//! - `end_frame` runs from a zero-size deferred element of the status bar item, which is
//!   painted last in every frame of every workspace window.

use std::cell::{Cell, RefCell};
use std::rc::{Rc, Weak};

use gpui::{
    AnyWindowHandle, App, Bounds, Element, ElementId, GlobalElementId, Hitbox, HitboxBehavior,
    InspectorElementId, IntoElement, LayoutId, Pixels, Size, Style, Window, point, px, size,
};
use gpui_component::WindowExt as _;

use crate::native::NativeWebView;

/// A native web view placed in a window.
pub struct Embedded {
    pub native: NativeWebView,
    window: AnyWindowHandle,
    /// Painted in the frame being drawn.
    painted: Cell<bool>,
    /// The bounds last applied to the native view.
    applied: Cell<Option<Bounds<Pixels>>>,
    /// Whether the native view is visible now.
    shown: Cell<bool>,
    /// The tab draws something of its own instead of the page (interstitial, error, menu).
    covered: Cell<bool>,
}

thread_local! {
    static EMBEDDED: RefCell<Vec<Weak<Embedded>>> = const { RefCell::new(Vec::new()) };
}

impl Embedded {
    pub fn new(native: NativeWebView, window: AnyWindowHandle) -> Rc<Self> {
        let embedded = Rc::new(Self {
            native,
            window,
            painted: Cell::new(false),
            applied: Cell::new(None),
            shown: Cell::new(false),
            covered: Cell::new(false),
        });
        EMBEDDED.with_borrow_mut(|views| {
            views.retain(|v| v.strong_count() > 0);
            views.push(Rc::downgrade(&embedded));
        });
        embedded
    }

    /// Hides the page while the tab shows something in its place.
    pub fn set_covered(&self, covered: bool) {
        self.covered.set(covered);
        if covered && self.shown.replace(false) {
            self.native.set_visible(false);
        }
    }

    pub fn is_shown(&self) -> bool {
        self.shown.get()
    }

    pub fn bounds(&self) -> Option<Bounds<Pixels>> {
        self.applied.get()
    }

    fn place(&self, bounds: Bounds<Pixels>) {
        self.painted.set(true);
        if self.applied.get() != Some(bounds) {
            self.applied.set(Some(bounds));
            self.native.set_bounds(bounds);
        }
    }
}

impl Drop for Embedded {
    fn drop(&mut self) {
        self.native.set_visible(false);
    }
}

/// Whether any web view lives in `window` (the status bar item skips its work otherwise).
pub fn has_views(window: &Window) -> bool {
    let handle = window.window_handle();
    EMBEDDED.with_borrow(|views| {
        views
            .iter()
            .filter_map(Weak::upgrade)
            .any(|v| v.window == handle)
    })
}

/// Shows the views painted in this frame of `window` and hides the rest.
pub fn end_frame(window: &mut Window, cx: &mut App) {
    let handle = window.window_handle();
    let views: Vec<Rc<Embedded>> = EMBEDDED.with_borrow_mut(|views| {
        views.retain(|v| v.strong_count() > 0);
        views
            .iter()
            .filter_map(Weak::upgrade)
            .filter(|v| v.window == handle)
            .collect()
    });
    if views.is_empty() {
        return;
    }
    let overlay = window.has_active_dialog(cx) || window.has_active_sheet(cx);
    let toasts = toast_area(window, cx);
    for view in views {
        let painted = view.painted.replace(false);
        let under_toast = match (toasts, view.applied.get()) {
            (Some(toasts), Some(bounds)) => toasts.intersects(&bounds),
            _ => false,
        };
        let visible = painted && !overlay && !under_toast && !view.covered.get();
        if view.shown.replace(visible) != visible {
            tracing::debug!(
                visible,
                painted,
                overlay,
                under_toast,
                "web view visibility"
            );
            view.native.set_visible(visible);
        }
    }
}

/// PNG snapshots of the web views shown in `window`, with their bounds (the screenshot harness
/// composites them over GPUI's frame, which can't see native views).
pub fn snapshots(
    window: &Window,
) -> impl std::future::Future<Output = Vec<(Bounds<Pixels>, Vec<u8>)>> + use<> {
    let handle = window.window_handle();
    let views: Vec<Rc<Embedded>> = EMBEDDED.with_borrow(|views| {
        views
            .iter()
            .filter_map(Weak::upgrade)
            .filter(|v| v.window == handle && v.shown.get())
            .collect()
    });
    let receivers: Vec<_> = views
        .iter()
        .filter_map(|view| {
            let bounds = view.applied.get()?;
            let (tx, rx) = futures::channel::oneshot::channel();
            view.native.snapshot(Box::new(move |png| {
                tx.send(png).ok();
            }));
            Some((bounds, rx))
        })
        .collect();
    async move {
        let mut shots = Vec::new();
        for (bounds, rx) in receivers {
            if let Ok(Some(png)) = rx.await {
                shots.push((bounds, png));
            }
        }
        shots
    }
}

/// Roughly where gpui-component stacks its toasts (top right), while any is shown.
fn toast_area(window: &mut Window, cx: &mut App) -> Option<Bounds<Pixels>> {
    let count = window.notifications(cx).len();
    if count == 0 {
        return None;
    }
    let viewport = window.viewport_size();
    // Wide enough for the default toast width plus its margin, tall enough for each toast.
    let width = px(420.0);
    let height = px(24.0 + 88.0 * count as f32);
    Some(Bounds::new(
        point(viewport.width - width, px(0.0)),
        size(width, height),
    ))
}

/// The element standing in for the page: the native view follows its bounds.
pub struct WebContent {
    embedded: Rc<Embedded>,
}

impl WebContent {
    pub fn new(embedded: Rc<Embedded>) -> Self {
        Self { embedded }
    }
}

impl IntoElement for WebContent {
    type Element = Self;

    fn into_element(self) -> Self::Element {
        self
    }
}

impl Element for WebContent {
    type RequestLayoutState = ();
    type PrepaintState = Option<Hitbox>;

    fn id(&self) -> Option<ElementId> {
        None
    }

    fn source_location(&self) -> Option<&'static std::panic::Location<'static>> {
        None
    }

    fn request_layout(
        &mut self,
        _: Option<&GlobalElementId>,
        _: Option<&InspectorElementId>,
        window: &mut Window,
        cx: &mut App,
    ) -> (LayoutId, Self::RequestLayoutState) {
        let style = Style {
            size: Size::full(),
            flex_grow: 1.0,
            flex_shrink: 1.0,
            ..Default::default()
        };
        (window.request_layout(style, [], cx), ())
    }

    fn prepaint(
        &mut self,
        _: Option<&GlobalElementId>,
        _: Option<&InspectorElementId>,
        bounds: Bounds<Pixels>,
        _: &mut Self::RequestLayoutState,
        window: &mut Window,
        _: &mut App,
    ) -> Self::PrepaintState {
        if bounds.size.width <= px(1.0) || bounds.size.height <= px(1.0) {
            return None;
        }
        // Clip to the visible part of the pane (a scrolled or partly hidden container).
        let visible = window.content_mask().bounds.intersect(&bounds);
        if visible.size.width <= px(1.0) || visible.size.height <= px(1.0) {
            return None;
        }
        self.embedded.place(visible);
        Some(window.insert_hitbox(visible, HitboxBehavior::BlockMouse))
    }

    fn paint(
        &mut self,
        _: Option<&GlobalElementId>,
        _: Option<&InspectorElementId>,
        _: Bounds<Pixels>,
        _: &mut Self::RequestLayoutState,
        _: &mut Self::PrepaintState,
        _: &mut Window,
        _: &mut App,
    ) {
    }
}
