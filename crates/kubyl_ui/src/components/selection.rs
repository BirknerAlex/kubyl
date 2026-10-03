//! The selectable text element behind [`crate::Selectable`].
//!
//! gpui-base orders the text copied from several participants by a document order that
//! defaults to 0 (a tie, so the order is arbitrary). Here every run takes the next number
//! of a per-frame counter in prepaint (tree order, which is reading order). The counter is
//! reset by a [`SelectionFrame`] element that sits before the content, as gpui-base's own
//! layer does for its text views.
//!
//! Runs inside a [`SelectionScope`] can also be selected all at once ([`select_all_in_scope`]).

use std::cell::{Cell, RefCell};
use std::collections::HashMap;
use std::ops::Range;
use std::rc::{Rc, Weak};

use gpui::{
    AnyElement, App, BorderStyle, Bounds, ContentMask, Corners, Edges, Element, ElementId,
    EntityId, GlobalElementId, HighlightStyle, Hitbox, HitboxBehavior, Hsla, InspectorElementId,
    IntoElement, LayoutId, MouseButton, MouseDownEvent, PaintQuad, Pixels, Point, SharedString,
    StyledText, Subscription, Window, WindowId, px, transparent_black,
};
use gpui_base::{TextSelection, TextSelectionHandle, TextSelectionRegistration, TextSelectionRun};

use crate::ActiveColors;

/// What one window has registered in the frame being drawn.
#[derive(Default)]
struct FrameState {
    /// A `SelectionFrame` numbers the runs; without one they are ordered by position.
    framed: bool,
    next_order: u64,
    scopes: Vec<OpenScope>,
    root_viewport: Bounds<Pixels>,
    scoped: Vec<ScopedRun>,
    scope_hitboxes: Vec<(ElementId, Hitbox)>,
    // What each run last painted, by participant: nothing needs a frame boundary to reset it,
    // and a run whose element state is gone (`Weak`) no longer counts.
    runs: HashMap<EntityId, RunState>,
    prune_at: usize,
    // The scope the last press started in (`Some(None)`: outside any); kept across frames.
    anchor_scope: Option<Option<ElementId>>,
    select_all: Option<SelectAll>,
}

impl FrameState {
    fn has_selected(&self) -> bool {
        self.runs
            .values()
            .any(|r| r.selected.is_some() && r.retained.strong_count() > 0)
    }
}

struct RunState {
    retained: Weak<Retained>,
    /// The projection reaches some text (maybe of another scope).
    any: bool,
    selected: Option<SelectedRun>,
}

#[derive(Clone)]
struct SelectedRun {
    order: u64,
    bounds: Bounds<Pixels>,
    line_height: Pixels,
    text: SharedString,
    range: Range<usize>,
}

struct OpenScope {
    id: ElementId,
    // The clip of the scope's surroundings: what a drag scrolls against.
    viewport: Bounds<Pixels>,
}

/// A select-all that is still on: runs that enter the scope join it.
#[derive(Clone)]
struct SelectAll {
    scope: ElementId,
    handles: Rc<RefCell<Vec<Weak<Retained>>>>,
}

struct ScopedRun {
    scope: ElementId,
    retained: Weak<Retained>,
    text: SharedString,
}

#[derive(Default)]
struct Frames(HashMap<WindowId, FrameState>);

impl gpui::Global for Frames {}

fn frame<R>(window: &Window, cx: &mut App, f: impl FnOnce(&mut FrameState) -> R) -> R {
    let id = window.window_handle().window_id();
    if !cx.default_global::<Frames>().0.contains_key(&id) {
        // A new window: forget the closed ones.
        let open: Vec<WindowId> = cx.windows().iter().map(|w| w.window_id()).collect();
        cx.global_mut::<Frames>()
            .0
            .retain(|id, _| open.contains(id));
    }
    f(cx.global_mut::<Frames>().0.entry(id).or_default())
}

/// How much per-window bookkeeping there is (growth shows as a leak).
#[cfg(test)]
fn state_size(window: &Window, cx: &mut App) -> usize {
    frame(window, cx, |frame| {
        let all = frame
            .select_all
            .as_ref()
            .map_or(0, |a| a.handles.borrow().len());
        frame.scoped.len() + frame.scope_hitboxes.len() + frame.runs.len() + all
    })
}

/// Starts the window's frame: resets the document order, so runs are numbered in tree (reading)
/// order. Render one as the first child of the workspace root, before any content. Without
/// one, runs are ordered by position instead.
pub struct SelectionFrame;

impl IntoElement for SelectionFrame {
    type Element = Self;

    fn into_element(self) -> Self::Element {
        self
    }
}

impl Element for SelectionFrame {
    type RequestLayoutState = ();
    type PrepaintState = ();

    fn id(&self) -> Option<ElementId> {
        Some("kubyl-selection-frame".into())
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
        (window.request_layout(gpui::Style::default(), [], cx), ())
    }

    fn prepaint(
        &mut self,
        _: Option<&GlobalElementId>,
        _: Option<&InspectorElementId>,
        _: Bounds<Pixels>,
        _: &mut Self::RequestLayoutState,
        window: &mut Window,
        cx: &mut App,
    ) -> Self::PrepaintState {
        let viewport = window.content_mask().bounds;
        frame(window, cx, |frame| {
            frame.framed = true;
            frame.next_order = 1;
            frame.scopes.clear();
            frame.root_viewport = viewport;
        });
    }

    fn paint(
        &mut self,
        _: Option<&GlobalElementId>,
        _: Option<&InspectorElementId>,
        _: Bounds<Pixels>,
        _: &mut Self::RequestLayoutState,
        _: &mut Self::PrepaintState,
        window: &mut Window,
        _: &mut App,
    ) {
        // A selection belongs to the scope its press started in; runs of other scopes (and
        // unscoped ones) don't take part. gpui-base's own scopes are driven by `Root`.
        window.on_mouse_event(move |event: &MouseDownEvent, phase, window, cx| {
            if !phase.capture() || event.button != MouseButton::Left {
                return;
            }
            frame(window, cx, |frame| {
                // Shift extends a selection, which keeps its scope.
                if event.modifiers.shift && frame.has_selected() {
                    return;
                }
                // The innermost (smallest) scope under the pointer.
                let scope = frame
                    .scope_hitboxes
                    .iter()
                    .filter(|(_, hitbox)| hitbox.is_hovered(window))
                    .min_by(|(_, a), (_, b)| {
                        let area = |bounds: &Bounds<Pixels>| {
                            f32::from(bounds.size.width) * f32::from(bounds.size.height)
                        };
                        area(&a.bounds).total_cmp(&area(&b.bounds))
                    })
                    .map(|(id, _)| id.clone());
                frame.anchor_scope = Some(scope);
            });
        });
    }
}

/// Groups the selectable runs of its child, so [`select_all_in_scope`] can select them.
pub struct SelectionScope {
    id: ElementId,
    child: AnyElement,
}

impl SelectionScope {
    pub fn new(id: impl Into<ElementId>, child: impl IntoElement) -> Self {
        Self {
            id: id.into(),
            child: child.into_any_element(),
        }
    }
}

impl IntoElement for SelectionScope {
    type Element = Self;

    fn into_element(self) -> Self::Element {
        self
    }
}

impl Element for SelectionScope {
    type RequestLayoutState = ();
    type PrepaintState = Hitbox;

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
        (self.child.request_layout(window, cx), ())
    }

    fn prepaint(
        &mut self,
        _: Option<&GlobalElementId>,
        _: Option<&InspectorElementId>,
        bounds: Bounds<Pixels>,
        _: &mut Self::RequestLayoutState,
        window: &mut Window,
        cx: &mut App,
    ) -> Self::PrepaintState {
        let viewport = window.content_mask().bounds;
        frame(window, cx, |frame| {
            frame.scoped.retain(|run| run.scope != self.id);
            frame.scope_hitboxes.retain(|(id, _)| *id != self.id);
            frame.scopes.push(OpenScope {
                id: self.id.clone(),
                viewport,
            });
        });
        self.child.prepaint(window, cx);
        frame(window, cx, |frame| {
            frame.scopes.pop();
        });
        // After the children, so none of them occludes it.
        let hitbox = window.insert_hitbox(bounds, HitboxBehavior::Normal);
        frame(window, cx, |frame| {
            frame.scope_hitboxes.push((self.id.clone(), hitbox.clone()))
        });
        hitbox
    }

    fn paint(
        &mut self,
        _: Option<&GlobalElementId>,
        _: Option<&InspectorElementId>,
        _: Bounds<Pixels>,
        _: &mut Self::RequestLayoutState,
        _: &mut Self::PrepaintState,
        window: &mut Window,
        cx: &mut App,
    ) {
        self.child.paint(window, cx);
    }
}

/// [`select_all_in_scope`] for the scope the last press started in.
pub(crate) fn select_all_in_last_scope(window: &mut Window, cx: &mut App) -> bool {
    let scope = frame(window, cx, |frame| frame.anchor_scope.clone().flatten());
    scope.is_some_and(|scope| select_all_in_scope(&scope, window, cx))
}

/// Selects all text of the scope's runs (copied in reading order with `⌘C`). Replaces any
/// other selection; the next mouse press clears it. Returns whether the scope had text.
pub fn select_all_in_scope(scope: &ElementId, window: &mut Window, cx: &mut App) -> bool {
    let runs: Vec<(Rc<Retained>, SharedString)> = frame(window, cx, |frame| {
        frame
            .scoped
            .iter()
            .filter(|run| &run.scope == scope)
            .filter_map(|run| Some((run.retained.upgrade()?, run.text.clone())))
            .collect()
    });
    if runs.is_empty() {
        return false;
    }
    TextSelection::clear(window, cx);
    let mut handles = Vec::new();
    for (retained, text) in runs {
        retained.handle.set_fallback_copy_text(text.to_string(), cx);
        retained.handle.set_local_selection(true, cx);
        handles.push(Rc::downgrade(&retained));
    }
    frame(window, cx, |frame| {
        frame.select_all = Some(SelectAll {
            scope: scope.clone(),
            handles: Rc::new(RefCell::new(handles)),
        })
    });
    window.refresh();
    true
}

/// The selected text, laid out like the UI: runs on one visual row share a line (separated by a
/// space), a new row starts a new line and a larger gap (a new section) leaves a blank line.
/// `None` when no selectable text is selected.
pub(crate) fn selected_text(window: &mut Window, cx: &mut App) -> Option<String> {
    let (runs, any) = frame(window, cx, |frame| {
        let live = frame
            .runs
            .values()
            .filter(|r| r.retained.strong_count() > 0);
        let any = live.clone().any(|r| r.any);
        let runs: Vec<SelectedRun> = live.filter_map(|r| r.selected.clone()).collect();
        (runs, any)
    });
    let text = if runs.is_empty() {
        // No selectable run is selected: text of other participants (rich text) or a stale
        // run of another scope, which `any` marks.
        if any {
            String::new()
        } else {
            TextSelection::selected_text(window, cx)
        }
    } else {
        assemble(&runs)
    };
    let text = clean(&text);
    (!text.is_empty()).then_some(text)
}

/// Whether text is selected, as painted: a drag over no characters (a click with a little
/// travel) is none.
pub(crate) fn has_selected_text(window: &mut Window, cx: &mut App) -> bool {
    let (painted, select_all) = frame(window, cx, |frame| {
        (
            frame.has_selected(),
            frame.select_all.as_ref().map(|all| all.handles.clone()),
        )
    });
    painted || select_all.is_some_and(|handles| any_local(&handles, cx))
}

/// Whether any live run of a select-all still has its local selection.
fn any_local(handles: &RefCell<Vec<Weak<Retained>>>, cx: &App) -> bool {
    handles
        .borrow()
        .iter()
        .filter_map(Weak::upgrade)
        .any(|r| r.handle.has_local_selection(cx))
}

/// Drops leading blank lines and trailing whitespace; the first line keeps its indentation.
fn clean(text: &str) -> String {
    let start = text
        .split_inclusive('\n')
        .take_while(|line| line.trim().is_empty())
        .map(str::len)
        .sum();
    text[start..].trim_end().to_string()
}

fn assemble(runs: &[SelectedRun]) -> String {
    let mut runs: Vec<&SelectedRun> = runs.iter().collect();
    runs.sort_by_key(|run| run.order);
    let mut out = String::new();
    // Vertical extent of the current row; `None` after a multi-line run.
    let mut row: Option<(Pixels, Pixels)> = None;
    let mut bottom = px(0.);
    for run in runs {
        let piece = &run.text[run.range.clone()];
        if piece.trim().is_empty() {
            continue;
        }
        let multi_line = run.bounds.size.height > run.line_height * 1.5;
        let top = run.bounds.top();
        // A wrapped value starts on its key's line: look at its first line.
        let same_row = row.is_some_and(|(row_top, row_bottom)| {
            let middle = top + run.bounds.size.height.min(run.line_height) / 2.;
            middle > row_top && middle < row_bottom
        });
        if same_row {
            out.push(' ');
        } else if !out.is_empty() {
            out.push_str(if top - bottom > run.line_height {
                "\n\n"
            } else {
                "\n"
            });
        }
        out.push_str(piece);
        let run_bottom = run.bounds.bottom();
        // What follows a multi-line run starts a new line.
        row = match (same_row, multi_line, row) {
            (_, true, _) => None,
            (true, _, Some((row_top, row_bottom))) => {
                Some((row_top.min(top), row_bottom.max(run_bottom)))
            }
            _ => Some((top, run_bottom)),
        };
        bottom = if same_row {
            bottom.max(run_bottom)
        } else {
            run_bottom
        };
    }
    out
}

/// Text that joins the window's text selection. Inherits the parent's text style.
pub(crate) struct SelectableLabel {
    id: ElementId,
    text: SharedString,
    styled_text: StyledText,
}

impl SelectableLabel {
    pub(crate) fn new(id: impl Into<ElementId>, text: impl Into<SharedString>) -> Self {
        let text = text.into();
        Self {
            id: id.into(),
            styled_text: StyledText::new(text.clone()),
            text,
        }
    }

    pub(crate) fn highlights(mut self, highlights: Vec<(Range<usize>, HighlightStyle)>) -> Self {
        if !highlights.is_empty() {
            self.styled_text = StyledText::new(self.text.clone()).with_highlights(highlights);
        }
        self
    }
}

impl IntoElement for SelectableLabel {
    type Element = Self;

    fn into_element(self) -> Self::Element {
        self
    }
}

pub(crate) struct RunInfo {
    order: u64,
    scope: Option<ElementId>,
}

pub(crate) struct Retained {
    handle: TextSelectionHandle,
    /// Taken by a laid-out element until it paints: a second element asking for the same
    /// state (an equal id under one parent) takes another one.
    claimed: Cell<bool>,
    _refresh: Subscription,
}

impl Element for SelectableLabel {
    type RequestLayoutState = Rc<Retained>;
    type PrepaintState = RunInfo;

    fn id(&self) -> Option<ElementId> {
        Some(self.id.clone())
    }

    fn source_location(&self) -> Option<&'static std::panic::Location<'static>> {
        None
    }

    fn request_layout(
        &mut self,
        global_id: Option<&GlobalElementId>,
        inspector_id: Option<&InspectorElementId>,
        window: &mut Window,
        cx: &mut App,
    ) -> (LayoutId, Self::RequestLayoutState) {
        let id = global_id.expect("SelectableLabel has a stable element id");
        // Safety net: equal ids under one parent (the same key in two rows) take the next
        // unclaimed state, so they never share a participant. Callers should still pass unique
        // ids; repeats are logged (`RUST_LOG=kubyl_ui=debug`).
        let text = self.text.clone();
        let mut retain = |global_id: &GlobalElementId, window: &mut Window| {
            window.with_element_state(global_id, |retained: Option<Rc<Retained>>, window| {
                let retained = retained.unwrap_or_else(|| {
                    let handle = TextSelectionHandle::new(text.to_string(), cx);
                    let refresh = handle.refresh_window_on_change(window, cx);
                    Rc::new(Retained {
                        handle,
                        claimed: Cell::new(false),
                        _refresh: refresh,
                    })
                });
                (retained.clone(), retained)
            })
        };
        let mut retained = retain(id, window);
        let mut repeat = 0;
        while retained.claimed.replace(true) {
            repeat += 1;
            tracing::debug!(id = ?id, "selectable text id repeats under one parent");
            retained = window.with_global_id(ElementId::Integer(repeat), |global_id, window| {
                retain(global_id, window)
            });
        }
        let (layout_id, ()) = self
            .styled_text
            .request_layout(global_id, inspector_id, window, cx);
        (layout_id, retained)
    }

    fn prepaint(
        &mut self,
        global_id: Option<&GlobalElementId>,
        inspector_id: Option<&InspectorElementId>,
        bounds: Bounds<Pixels>,
        retained: &mut Self::RequestLayoutState,
        window: &mut Window,
        cx: &mut App,
    ) -> Self::PrepaintState {
        self.styled_text
            .prepaint(global_id, inspector_id, bounds, &mut (), window, cx);
        let mut hitbox = window.insert_hitbox(bounds, HitboxBehavior::Normal);
        // gpui-base scrolls drags against the hitbox's content mask. A truncated value clips
        // to its own row (a mask a line or two high), so every drag in it would scroll; only
        // then use the surrounding viewport. Any other mask is a scroll container's: keep it.
        let line_height = window.text_style().line_height_in_pixels(window.rem_size());
        if window.content_mask().bounds.size.height <= line_height * 2. {
            let viewport = frame(window, cx, |frame| {
                frame
                    .scopes
                    .last()
                    .map_or(frame.root_viewport, |scope| scope.viewport)
            });
            if viewport.size.width > px(0.) && viewport.size.height > px(0.) {
                hitbox.content_mask = ContentMask { bounds: viewport };
            }
        }
        let (order, scope) = frame(window, cx, |frame| {
            let order = if frame.framed {
                frame.next_order += 1;
                frame.next_order - 1
            } else {
                // Reading order as position: top to bottom, then left to right. A counter
                // nothing resets would renumber every frame and never settle.
                let y = f32::from(bounds.origin.y).max(0.) as u64;
                let x = f32::from(bounds.origin.x).max(0.) as u64;
                (y << 24) | x.min(0xff_ffff)
            };
            let scope = frame.scopes.last().map(|scope| scope.id.clone());
            if let Some(scope) = &scope {
                frame.scoped.push(ScopedRun {
                    scope: scope.clone(),
                    retained: Rc::downgrade(retained),
                    text: self.text.clone(),
                });
            }
            (order, scope)
        });
        // A run that appears while its scope is selected (new handle after a re-render) joins.
        if let Some(scope) = scope.clone() {
            let handles = frame(window, cx, |frame| {
                frame
                    .select_all
                    .as_ref()
                    .filter(|all| all.scope == scope)
                    .map(|all| all.handles.clone())
            });
            let active = handles.as_ref().is_some_and(|h| any_local(h, cx));
            if active && !retained.handle.has_local_selection(cx) {
                retained.handle.set_local_selection(true, cx);
                if let Some(handles) = &handles {
                    let mut handles = handles.borrow_mut();
                    handles.retain(|h| h.strong_count() > 0);
                    handles.push(Rc::downgrade(retained));
                }
            } else if !active {
                frame(window, cx, |frame| {
                    if frame.select_all.as_ref().is_some_and(|a| a.scope == scope) {
                        frame.select_all = None;
                    }
                });
            }
        }
        retained.handle.register(
            TextSelectionRegistration::new(hitbox.clone(), bounds)
                .with_document_order(order)
                .with_text_bounds(vec![bounds]),
            window,
            cx,
        );
        RunInfo { order, scope }
    }

    fn paint(
        &mut self,
        global_id: Option<&GlobalElementId>,
        inspector_id: Option<&InspectorElementId>,
        bounds: Bounds<Pixels>,
        retained: &mut Self::RequestLayoutState,
        info: &mut Self::PrepaintState,
        window: &mut Window,
        cx: &mut App,
    ) {
        let layout = self.styled_text.layout().clone();
        // gpui-base only projects a selection onto text equal to the laid-out text, which a
        // truncated run (cut and ended with an ellipsis) isn't: register what is shown, and
        // map the selection back to the full text below.
        let shown: Option<SharedString> =
            (layout.len() != self.text.len()).then(|| layout.text().into());
        let projection = retained.handle.update_runs(
            &[TextSelectionRun::new(
                shown.clone().unwrap_or_else(|| self.text.clone()),
                layout.clone(),
                bounds,
            )
            .with_document_order(info.order)],
            cx,
        );
        let color = cx.colors().accent.opacity(0.3);
        let mut any = false;
        let selected = if retained.handle.has_local_selection(cx) {
            // Keeps the copy text current; `update_runs` cached an empty projection.
            retained
                .handle
                .set_fallback_copy_text(self.text.to_string(), cx);
            paint_selection(&layout, 0..layout.len(), color, window);
            Some(0..self.text.len())
        } else {
            let range = projection
                .ranges()
                .iter()
                .flatten()
                .find(|range| !range.is_empty())
                .cloned();
            // A drag only selects within the scope it started in.
            any = range.is_some();
            let own_scope = frame(window, cx, |frame| {
                frame
                    .anchor_scope
                    .as_ref()
                    .is_none_or(|anchor| *anchor == info.scope)
            });
            let range = range.filter(|_| own_scope);
            if let Some(range) = &range {
                paint_selection(&layout, range.clone(), color, window);
            }
            match (&shown, range) {
                (Some(shown), Some(range)) => {
                    Some(full_range(shown, &self.text, range)).filter(|r| !r.is_empty())
                }
                (_, range) => range,
            }
        };
        let line_height = layout.line_height();
        let selected = selected.map(|range| SelectedRun {
            order: info.order,
            bounds,
            line_height,
            text: self.text.clone(),
            range,
        });
        let state = RunState {
            retained: Rc::downgrade(retained),
            any: any || selected.is_some(),
            selected,
        };
        frame(window, cx, |frame| {
            frame.runs.insert(retained.handle.entity_id(), state);
            if frame.runs.len() > frame.prune_at {
                frame.runs.retain(|_, r| r.retained.strong_count() > 0);
                frame.prune_at = (frame.runs.len() * 2).max(1024);
            }
        });
        retained.claimed.set(false);
        self.styled_text.paint(
            global_id,
            inspector_id,
            bounds,
            &mut (),
            &mut (),
            window,
            cx,
        );
    }
}

/// A selection of a truncated run's shown text (`prefix…`) as a range of its full text: what
/// lies in the prefix maps 1:1; reaching the ellipsis takes the rest of the text.
fn full_range(shown: &str, full: &str, range: Range<usize>) -> Range<usize> {
    let visible = shown.strip_suffix('…').map_or(shown.len(), str::len);
    let start = range.start.min(visible);
    let end = if range.end >= shown.len() {
        full.len()
    } else {
        range.end.min(visible)
    };
    start..end.max(start)
}

fn paint_selection(
    layout: &gpui::TextLayout,
    range: Range<usize>,
    color: Hsla,
    window: &mut Window,
) {
    let (Some(start), Some(end)) = (
        layout.position_for_index(range.start),
        layout.position_for_index(range.end),
    ) else {
        return;
    };
    for bounds in quad_bounds(start, end, layout.bounds(), layout.line_height()) {
        window.paint_quad(PaintQuad {
            bounds,
            background: color.into(),
            corner_radii: Corners::default(),
            border_widths: Edges::default(),
            border_color: transparent_black(),
            border_style: BorderStyle::default(),
        });
    }
}

fn quad_bounds(
    start: Point<Pixels>,
    end: Point<Pixels>,
    bounds: Bounds<Pixels>,
    line_height: Pixels,
) -> Vec<Bounds<Pixels>> {
    if start.y == end.y {
        return vec![Bounds::from_corners(
            start,
            Point::new(end.x, end.y + line_height),
        )];
    }
    let mut quads = vec![Bounds::from_corners(
        start,
        Point::new(bounds.right(), start.y + line_height),
    )];
    if end.y > start.y + line_height {
        quads.push(Bounds::from_corners(
            Point::new(bounds.left(), start.y + line_height),
            Point::new(bounds.right(), end.y),
        ));
    }
    quads.push(Bounds::from_corners(
        Point::new(bounds.left(), end.y),
        Point::new(end.x, end.y + line_height),
    ));
    quads
}

#[cfg(test)]
mod tests {
    use std::cell::Cell;

    use crate::Chip;
    use gpui::{
        Context, Modifiers, MouseButton, MouseDownEvent, MouseMoveEvent, MouseUpEvent, Pixels,
        Point, Render, TestAppContext, VisualTestContext, Window, div, point, prelude::*, px,
    };

    use super::*;

    const SCOPE: &str = "scope";

    // Three runs stacked like the details rows, plus one outside the scope.
    struct Stacked;

    impl Render for Stacked {
        fn render(&mut self, _: &mut Window, _: &mut Context<Self>) -> impl IntoElement {
            let row = |id: &'static str, text: &'static str| {
                div()
                    .h(px(20.))
                    .w(px(300.))
                    .child(SelectableLabel::new(id, text))
            };
            div()
                .size_full()
                .child(SelectionFrame)
                .child(SelectionScope::new(
                    SCOPE,
                    div()
                        .child(row("a", "docker.io/grafana/alloy v1.16.1"))
                        .child(row("b", "two"))
                        .child(row("c", "three")),
                ))
                .child(row("outside", "elsewhere"))
        }
    }

    fn setup(cx: &mut TestAppContext) -> &mut VisualTestContext {
        cx.update(|cx| {
            gpui_component::init(cx);
            crate::Theme::dark().apply(cx);
        });
        let (_, cx) = cx.add_window_view(|window, cx| {
            let view = cx.new(|_| Stacked);
            gpui_component::Root::new(view, window, cx)
        });
        cx.run_until_parked();
        cx
    }

    fn press(cx: &mut VisualTestContext, at: Point<Pixels>, count: usize) {
        cx.simulate_event(MouseDownEvent {
            position: at,
            modifiers: Modifiers::default(),
            button: MouseButton::Left,
            click_count: count,
            first_mouse: false,
        });
        cx.simulate_event(MouseUpEvent {
            position: at,
            modifiers: Modifiers::default(),
            button: MouseButton::Left,
            click_count: count,
        });
        cx.run_until_parked();
    }

    fn drag(cx: &mut VisualTestContext, from: Point<Pixels>, to: Point<Pixels>) {
        cx.simulate_event(MouseDownEvent {
            position: from,
            modifiers: Modifiers::default(),
            button: MouseButton::Left,
            click_count: 1,
            first_mouse: false,
        });
        cx.simulate_event(MouseMoveEvent {
            position: to,
            modifiers: Modifiers::default(),
            pressed_button: Some(MouseButton::Left),
        });
        cx.simulate_event(MouseUpEvent {
            position: to,
            modifiers: Modifiers::default(),
            button: MouseButton::Left,
            click_count: 1,
        });
        cx.run_until_parked();
    }

    fn selected(cx: &mut VisualTestContext) -> String {
        cx.update(|window, cx| {
            window.refresh();
            let _ = window.draw(cx);
            super::selected_text(window, cx).unwrap_or_default()
        })
    }

    #[gpui::test]
    fn drag_within_a_run(cx: &mut TestAppContext) {
        let cx = setup(cx);
        drag(cx, point(px(1.), px(6.)), point(px(40.), px(6.)));
        let text = selected(cx);
        assert!(!text.is_empty() && "docker.io/grafana/alloy v1.16.1".starts_with(&text));
    }

    #[gpui::test]
    fn double_click_selects_a_word(cx: &mut TestAppContext) {
        let cx = setup(cx);
        press(cx, point(px(5.), px(6.)), 1);
        press(cx, point(px(5.), px(6.)), 2);
        assert_eq!(selected(cx), "docker");
    }

    #[gpui::test(iterations = 20)]
    fn drag_across_three_runs_copies_in_reading_order(cx: &mut TestAppContext) {
        let cx = setup(cx);
        drag(cx, point(px(1.), px(6.)), point(px(100.), px(46.)));
        let text = selected(cx);
        let lines: Vec<&str> = text.lines().collect();
        assert_eq!(lines.len(), 3, "{text:?}");
        assert!(lines[0].ends_with("v1.16.1"), "{text:?}");
        assert_eq!(&lines[1..], ["two", "three"]);
    }

    #[gpui::test(iterations = 20)]
    fn reverse_drag_copies_in_reading_order(cx: &mut TestAppContext) {
        let cx = setup(cx);
        drag(cx, point(px(100.), px(46.)), point(px(1.), px(6.)));
        let text = selected(cx);
        let lines: Vec<&str> = text.lines().collect();
        assert_eq!(lines.len(), 3, "{text:?}");
        assert_eq!(&lines[1..], ["two", "three"]);
    }

    #[gpui::test(iterations = 20)]
    fn select_all_copies_the_scope_in_reading_order(cx: &mut TestAppContext) {
        let cx = setup(cx);
        let selected_any = cx.update(|window, cx| select_all_in_scope(&SCOPE.into(), window, cx));
        assert!(selected_any);
        assert_eq!(selected(cx), "docker.io/grafana/alloy v1.16.1\ntwo\nthree");
    }

    #[gpui::test]
    fn a_press_clears_select_all(cx: &mut TestAppContext) {
        let cx = setup(cx);
        cx.update(|window, cx| select_all_in_scope(&SCOPE.into(), window, cx));
        press(cx, point(px(5.), px(100.)), 1);
        assert_eq!(selected(cx), "");
    }

    #[gpui::test]
    fn select_all_without_a_scope_selects_nothing(cx: &mut TestAppContext) {
        let cx = setup(cx);
        assert!(!cx.update(|window, cx| select_all_in_scope(&"missing".into(), window, cx)));
        assert_eq!(selected(cx), "");
    }

    // One multi-line run, like the Describe tab.
    struct Lines;

    const LINES: &str = "Name:  alloy-logs\nNamespace:  logging\n  Image:  docker.io/grafana/alloy";

    impl Render for Lines {
        fn render(&mut self, _: &mut Window, _: &mut Context<Self>) -> impl IntoElement {
            div()
                .size_full()
                .child(SelectionFrame)
                .child(SelectionScope::new(
                    SCOPE,
                    div()
                        .w(px(400.))
                        .line_height(px(20.))
                        .whitespace_nowrap()
                        .child(SelectableLabel::new("describe", LINES)),
                ))
        }
    }

    fn setup_lines(cx: &mut TestAppContext) -> &mut VisualTestContext {
        cx.update(|cx| {
            gpui_component::init(cx);
            crate::Theme::dark().apply(cx);
        });
        let (_, cx) = cx.add_window_view(|window, cx| {
            let view = cx.new(|_| Lines);
            gpui_component::Root::new(view, window, cx)
        });
        cx.run_until_parked();
        cx
    }

    #[gpui::test]
    fn triple_click_selects_one_line_of_a_multi_line_run(cx: &mut TestAppContext) {
        let cx = setup_lines(cx);
        press(cx, point(px(5.), px(26.)), 1);
        press(cx, point(px(5.), px(26.)), 2);
        press(cx, point(px(5.), px(26.)), 3);
        assert_eq!(selected(cx), "Namespace:  logging");
    }

    #[gpui::test]
    fn drag_across_lines_keeps_newlines_and_indent(cx: &mut TestAppContext) {
        let cx = setup_lines(cx);
        drag(cx, point(px(0.), px(26.)), point(px(399.), px(46.)));
        assert_eq!(
            selected(cx),
            "Namespace:  logging\n  Image:  docker.io/grafana/alloy"
        );
    }

    #[gpui::test]
    fn select_all_copies_a_multi_line_run(cx: &mut TestAppContext) {
        let cx = setup_lines(cx);
        assert!(cx.update(|window, cx| select_all_in_scope(&SCOPE.into(), window, cx)));
        assert_eq!(selected(cx), LINES);
    }

    // Identical labels under explicit ids stay separate participants.
    struct Duplicates {
        age: Rc<Cell<u32>>,
        keyed_by_text: bool,
    }

    impl Render for Duplicates {
        fn render(&mut self, _: &mut Window, _: &mut Context<Self>) -> impl IntoElement {
            let age = format!("age {}", self.age.get());
            let age_id: ElementId = if self.keyed_by_text {
                ElementId::Name(age.clone().into())
            } else {
                "age".into()
            };
            div()
                .size_full()
                .child(SelectionFrame)
                .child(SelectionScope::new(
                    SCOPE,
                    div()
                        .child(
                            div()
                                .h(px(20.))
                                .child(Chip::new("same").selectable_as(("c", 0u64))),
                        )
                        .child(
                            div()
                                .h(px(20.))
                                .child(Chip::new("same").selectable_as(("c", 1u64))),
                        )
                        .child(
                            div()
                                .h(px(20.))
                                .child(SelectableLabel::new("third", "third")),
                        )
                        .child(div().h(px(20.)).child(SelectableLabel::new(age_id, age))),
                ))
        }
    }

    fn setup_duplicates(
        age: Rc<Cell<u32>>,
        keyed_by_text: bool,
        cx: &mut TestAppContext,
    ) -> &mut VisualTestContext {
        cx.update(|cx| {
            gpui_component::init(cx);
            crate::Theme::dark().apply(cx);
        });
        let (_, cx) = cx.add_window_view(move |window, cx| {
            let view = cx.new(|_| Duplicates { age, keyed_by_text });
            gpui_component::Root::new(view, window, cx)
        });
        cx.run_until_parked();
        cx
    }

    #[gpui::test]
    fn duplicate_labels_with_distinct_ids_select_all(cx: &mut TestAppContext) {
        let cx = setup_duplicates(Rc::new(Cell::new(1)), false, cx);
        assert!(cx.update(|window, cx| select_all_in_scope(&SCOPE.into(), window, cx)));
        assert_eq!(selected(cx), "same\nsame\nthird\nage 1");
    }

    #[gpui::test]
    fn duplicate_labels_with_distinct_ids_drag_separately(cx: &mut TestAppContext) {
        let cx = setup_duplicates(Rc::new(Cell::new(1)), false, cx);
        drag(cx, point(px(10.), px(6.)), point(px(200.), px(6.)));
        let text = selected(cx);
        assert!(!text.is_empty() && "same".ends_with(&text), "{text:?}");
    }

    #[gpui::test]
    fn a_run_that_changes_its_id_joins_select_all(cx: &mut TestAppContext) {
        let age = Rc::new(Cell::new(1));
        let cx = setup_duplicates(age.clone(), true, cx);
        cx.update(|window, cx| select_all_in_scope(&SCOPE.into(), window, cx));
        age.set(2);
        let _ = selected(cx);
        // The replaced run is swept after the frame.
        cx.run_until_parked();
        assert_eq!(selected(cx), "same\nsame\nthird\nage 2");
    }

    #[gpui::test]
    fn truncated_text_has_a_position_for_the_layout_length(cx: &mut TestAppContext) {
        struct Truncated(Option<StyledText>);

        impl Render for Truncated {
            fn render(&mut self, _: &mut Window, _: &mut Context<Self>) -> impl IntoElement {
                div().w(px(30.)).truncate().children(self.0.take())
            }
        }

        let text = "docker.io/grafana/alloy v1.16.1";
        let styled = StyledText::new(text);
        let layout = styled.layout().clone();
        let (_, cx) = cx.add_window_view(move |_, _| Truncated(Some(styled)));
        cx.update(|window, cx| {
            let _ = window.draw(cx);
        });
        assert!(layout.position_for_index(text.len()).is_none());
        assert!(layout.position_for_index(layout.len()).is_some());
    }

    // A truncated value in a scroll container taller than its viewport.
    struct Scrolling(gpui::ScrollHandle);

    impl Render for Scrolling {
        fn render(&mut self, _: &mut Window, _: &mut Context<Self>) -> impl IntoElement {
            div().size_full().child(SelectionFrame).child(
                div()
                    .h(px(100.))
                    .w(px(300.))
                    .overflow_hidden()
                    .child(SelectionScope::new(
                        SCOPE,
                        div()
                            .id("scroll")
                            .size_full()
                            .overflow_y_scroll()
                            .track_scroll(&self.0)
                            .child(div().h(px(80.)))
                            .child(div().h(px(18.)).w(px(300.)).truncate().child(
                                SelectableLabel::new("v", "docker.io/grafana/alloy v1.16.1"),
                            ))
                            .child(div().h(px(400.))),
                    )),
            )
        }
    }

    fn setup_scrolling(cx: &mut TestAppContext) -> (&mut VisualTestContext, gpui::ScrollHandle) {
        cx.update(|cx| {
            gpui_component::init(cx);
            crate::Theme::dark().apply(cx);
        });
        let handle = gpui::ScrollHandle::new();
        let view_handle = handle.clone();
        let (_, cx) = cx.add_window_view(move |window, cx| {
            let view = cx.new(|_| Scrolling(view_handle));
            gpui_component::Root::new(view, window, cx)
        });
        cx.run_until_parked();
        cx.update(|window, cx| {
            let _ = window.draw(cx);
        });
        handle.set_offset(point(px(0.), px(-40.)));
        cx.update(|window, cx| {
            window.refresh();
            let _ = window.draw(cx);
        });
        (cx, handle)
    }

    fn hold_drag(cx: &mut VisualTestContext, from: Point<Pixels>, to: Point<Pixels>) {
        cx.simulate_event(MouseDownEvent {
            position: from,
            modifiers: Modifiers::default(),
            button: MouseButton::Left,
            click_count: 1,
            first_mouse: false,
        });
        cx.simulate_event(MouseMoveEvent {
            position: to,
            modifiers: Modifiers::default(),
            pressed_button: Some(MouseButton::Left),
        });
        cx.executor()
            .advance_clock(std::time::Duration::from_millis(300));
        cx.run_until_parked();
    }

    fn release(cx: &mut VisualTestContext, at: Point<Pixels>) {
        cx.simulate_event(MouseUpEvent {
            position: at,
            modifiers: Modifiers::default(),
            button: MouseButton::Left,
            click_count: 1,
        });
        cx.run_until_parked();
    }

    #[gpui::test]
    fn dragging_inside_a_truncated_value_does_not_scroll(cx: &mut TestAppContext) {
        let (cx, scroll) = setup_scrolling(cx);
        let offset = scroll.offset();
        hold_drag(cx, point(px(2.), px(48.)), point(px(100.), px(48.)));
        assert_eq!(scroll.offset(), offset);
        release(cx, point(px(100.), px(48.)));
        assert!(!selected(cx).is_empty());
    }

    #[gpui::test]
    fn dragging_past_the_container_scrolls_it(cx: &mut TestAppContext) {
        let (cx, scroll) = setup_scrolling(cx);
        let offset = scroll.offset();
        hold_drag(cx, point(px(2.), px(48.)), point(px(100.), px(140.)));
        assert!(scroll.offset().y < offset.y, "{:?}", scroll.offset());
        release(cx, point(px(100.), px(140.)));
    }

    // Two panels side by side and an unscoped row below, like the center pane and the dock.
    struct Panels;

    impl Render for Panels {
        fn render(&mut self, _: &mut Window, _: &mut Context<Self>) -> impl IntoElement {
            let col = |scope: &'static str, prefix: &'static str| {
                let row = move |n: u32| {
                    div().h(px(20.)).w(px(300.)).child(SelectableLabel::new(
                        SharedString::from(format!("{prefix}{n}")),
                        format!("{prefix}{n}"),
                    ))
                };
                SelectionScope::new(scope, div().child(row(1)).child(row(2)).child(row(3)))
            };
            div()
                .size_full()
                .child(SelectionFrame)
                .child(
                    div()
                        .flex()
                        .child(col("left", "a"))
                        .child(col("right", "b")),
                )
                .child(
                    div()
                        .h(px(20.))
                        .w(px(300.))
                        .child(SelectableLabel::new("free", "free")),
                )
        }
    }

    fn setup_panels(cx: &mut TestAppContext) -> &mut VisualTestContext {
        cx.update(|cx| {
            gpui_component::init(cx);
            crate::Theme::dark().apply(cx);
        });
        let (_, cx) = cx.add_window_view(|window, cx| {
            let view = cx.new(|_| Panels);
            gpui_component::Root::new(view, window, cx)
        });
        cx.run_until_parked();
        cx
    }

    #[gpui::test]
    fn a_drag_stays_in_the_scope_it_started_in(cx: &mut TestAppContext) {
        let cx = setup_panels(cx);
        drag(cx, point(px(1.), px(6.)), point(px(320.), px(46.)));
        let text = selected(cx);
        assert!(text.starts_with('a') && !text.contains('b'), "{text:?}");
        drag(cx, point(px(301.), px(6.)), point(px(20.), px(46.)));
        let text = selected(cx);
        assert!(text.starts_with('b') && !text.contains('a'), "{text:?}");
    }

    #[gpui::test]
    fn unscoped_text_and_scopes_do_not_mix(cx: &mut TestAppContext) {
        let cx = setup_panels(cx);
        drag(cx, point(px(1.), px(66.)), point(px(20.), px(6.)));
        let text = selected(cx);
        assert!(!text.contains('a') && !text.contains('b'), "{text:?}");
        drag(cx, point(px(1.), px(6.)), point(px(20.), px(66.)));
        let text = selected(cx);
        assert!(!text.contains("free"), "{text:?}");
    }

    // Rows like the details summary: key/value, chips, and a section further down.
    struct Formatted;

    impl Render for Formatted {
        fn render(&mut self, _: &mut Window, _: &mut Context<Self>) -> impl IntoElement {
            let label = |id: &'static str, text: &'static str| SelectableLabel::new(id, text);
            div()
                .size_full()
                .child(SelectionFrame)
                .child(SelectionScope::new(
                    SCOPE,
                    div()
                        .flex()
                        .flex_col()
                        .child(
                            div()
                                .flex()
                                .h(px(20.))
                                .gap(px(8.))
                                .child(div().w(px(100.)).child(label("k", "Firing since")))
                                .child(div().child(label("v", "10h 59m"))),
                        )
                        .child(
                            div()
                                .flex()
                                .h(px(20.))
                                .gap(px(4.))
                                .child(div().child(label("c1", "chip-one")))
                                .child(div().child(label("c2", "chip-two"))),
                        )
                        .child(div().h(px(60.)))
                        .child(div().h(px(20.)).child(label("s1", "Section")))
                        .child(div().h(px(20.)).child(label("s2", "next"))),
                ))
        }
    }

    fn setup_formatted(cx: &mut TestAppContext) -> &mut VisualTestContext {
        cx.update(|cx| {
            gpui_component::init(cx);
            crate::Theme::dark().apply(cx);
        });
        let (_, cx) = cx.add_window_view(|window, cx| {
            let view = cx.new(|_| Formatted);
            gpui_component::Root::new(view, window, cx)
        });
        cx.run_until_parked();
        cx
    }

    #[gpui::test]
    fn select_all_reads_like_the_ui(cx: &mut TestAppContext) {
        let cx = setup_formatted(cx);
        assert!(cx.update(|window, cx| select_all_in_scope(&SCOPE.into(), window, cx)));
        assert_eq!(
            selected(cx),
            "Firing since 10h 59m\nchip-one chip-two\n\nSection\nnext"
        );
    }

    #[gpui::test]
    fn a_drag_reads_like_the_ui(cx: &mut TestAppContext) {
        let cx = setup_formatted(cx);
        drag(cx, point(px(1.), px(6.)), point(px(110.), px(26.)));
        let text = selected(cx);
        assert!(
            text.starts_with("Firing since 10h 59m\nchip-one chi"),
            "{text:?}"
        );
        assert!(text.ends_with(|c: char| c != ' ' && c != '\n'), "{text:?}");
    }

    #[test]
    fn copied_text_keeps_the_first_line_indent() {
        assert_eq!(clean("\n  \n   indented\nnext  \n"), "   indented\nnext");
    }

    // The same id used twice under one parent must not make one participant.
    struct SameId;

    impl Render for SameId {
        fn render(&mut self, _: &mut Window, _: &mut Context<Self>) -> impl IntoElement {
            div()
                .size_full()
                .child(SelectionFrame)
                .child(SelectionScope::new(
                    SCOPE,
                    div()
                        .child(div().h(px(20.)).child(SelectableLabel::new("dup", "same")))
                        .child(div().h(px(20.)).child(SelectableLabel::new("dup", "same")))
                        .child(div().h(px(20.)).child(SelectableLabel::new("dup", "third"))),
                ))
        }
    }

    #[gpui::test]
    fn equal_ids_under_one_parent_stay_separate(cx: &mut TestAppContext) {
        cx.update(|cx| {
            gpui_component::init(cx);
            crate::Theme::dark().apply(cx);
        });
        let (_, cx) = cx.add_window_view(|window, cx| {
            let view = cx.new(|_| SameId);
            gpui_component::Root::new(view, window, cx)
        });
        cx.run_until_parked();
        assert!(cx.update(|window, cx| select_all_in_scope(&SCOPE.into(), window, cx)));
        assert_eq!(selected(cx), "same\nsame\nthird");
        drag(cx, point(px(1.), px(6.)), point(px(30.), px(6.)));
        assert!(!selected(cx).is_empty());
    }

    #[gpui::test]
    fn select_all_follows_the_scope_of_the_last_press(cx: &mut TestAppContext) {
        let cx = setup_panels(cx);
        assert!(!cx.update(select_all_in_last_scope));
        press(cx, point(px(301.), px(6.)), 1);
        assert!(cx.update(select_all_in_last_scope));
        assert_eq!(selected(cx), "b1\nb2\nb3");
        press(cx, point(px(1.), px(66.)), 1);
        assert!(!cx.update(select_all_in_last_scope));
    }

    #[gpui::test]
    fn a_click_with_a_little_travel_is_not_a_text_selection(cx: &mut TestAppContext) {
        let cx = setup(cx);
        drag(cx, point(px(5.), px(6.)), point(px(6.), px(6.)));
        let _ = selected(cx);
        assert!(!cx.update(crate::has_text_selection));
        drag(cx, point(px(5.), px(6.)), point(px(80.), px(6.)));
        let _ = selected(cx);
        assert!(cx.update(crate::has_text_selection));
    }

    struct Nested;

    impl Render for Nested {
        fn render(&mut self, _: &mut Window, _: &mut Context<Self>) -> impl IntoElement {
            let row = |id: &'static str| {
                div()
                    .h(px(20.))
                    .w(px(300.))
                    .child(SelectableLabel::new(id, id))
            };
            div()
                .size_full()
                .child(SelectionFrame)
                .child(SelectionScope::new(
                    "outer",
                    div().child(row("outer-one")).child(SelectionScope::new(
                        "inner",
                        div().child(row("inner-one")).child(row("inner-two")),
                    )),
                ))
        }
    }

    #[gpui::test]
    fn a_press_belongs_to_the_innermost_scope(cx: &mut TestAppContext) {
        cx.update(|cx| {
            gpui_component::init(cx);
            crate::Theme::dark().apply(cx);
        });
        let (_, cx) = cx.add_window_view(|window, cx| {
            let view = cx.new(|_| Nested);
            gpui_component::Root::new(view, window, cx)
        });
        cx.run_until_parked();
        drag(cx, point(px(1.), px(26.)), point(px(40.), px(46.)));
        assert!(selected(cx).starts_with("inner"), "{:?}", selected(cx));
        assert!(cx.update(select_all_in_last_scope));
        assert_eq!(selected(cx), "inner-one\ninner-two");
    }

    const LONG: &str = "docker.io/grafana/alloy v1.16.1";

    // Rows a / truncated value / c.
    struct Clipped;

    impl Render for Clipped {
        fn render(&mut self, _: &mut Window, _: &mut Context<Self>) -> impl IntoElement {
            div()
                .size_full()
                .child(SelectionFrame)
                .child(SelectionScope::new(
                    SCOPE,
                    div()
                        .child(
                            div()
                                .h(px(20.))
                                .w(px(300.))
                                .child(SelectableLabel::new("a", "alpha")),
                        )
                        .child(
                            div()
                                .h(px(20.))
                                .w(px(60.))
                                .truncate()
                                .child(SelectableLabel::new("b", LONG)),
                        )
                        .child(
                            div()
                                .h(px(20.))
                                .w(px(300.))
                                .child(SelectableLabel::new("c", "gamma")),
                        ),
                ))
        }
    }

    fn setup_clipped(cx: &mut TestAppContext) -> &mut VisualTestContext {
        cx.update(|cx| {
            gpui_component::init(cx);
            crate::Theme::dark().apply(cx);
        });
        let (_, cx) = cx.add_window_view(|window, cx| {
            let view = cx.new(|_| Clipped);
            gpui_component::Root::new(view, window, cx)
        });
        cx.run_until_parked();
        cx
    }

    #[gpui::test]
    fn a_drag_over_a_truncated_run_copies_its_full_text(cx: &mut TestAppContext) {
        let cx = setup_clipped(cx);
        drag(cx, point(px(1.), px(6.)), point(px(40.), px(46.)));
        let text = selected(cx);
        assert!(text.starts_with(&format!("alpha\n{LONG}\nga")), "{text:?}");
    }

    #[gpui::test]
    fn a_drag_inside_a_truncated_run_copies_the_visible_prefix(cx: &mut TestAppContext) {
        let cx = setup_clipped(cx);
        drag(cx, point(px(1.), px(26.)), point(px(20.), px(26.)));
        let text = selected(cx);
        assert!(!text.is_empty() && LONG.starts_with(&text), "{text:?}");
    }

    #[test]
    fn selections_of_shown_text_map_to_the_full_text() {
        let full = "docker.io/grafana";
        let shown = "docker…";
        assert_eq!(full_range(shown, full, 2..5), 2..5);
        assert_eq!(full_range(shown, full, 2..7), 2..6);
        assert_eq!(full_range(shown, full, 2..shown.len()), 2..full.len());
        assert_eq!(full_range(shown, full, 0..shown.len()), 0..full.len());
        assert_eq!(full_range(shown, full, 6..shown.len()), 6..full.len());
        assert_eq!(full_range("plain", "plain", 1..3), 1..3);
    }

    // Rows without a `SelectionFrame`: nothing resets the per-frame bookkeeping.
    struct Frameless;

    impl Render for Frameless {
        fn render(&mut self, _: &mut Window, _: &mut Context<Self>) -> impl IntoElement {
            let row = |id: &'static str, text: &'static str| {
                div()
                    .h(px(20.))
                    .w(px(300.))
                    .child(SelectableLabel::new(id, text))
            };
            div().size_full().child(SelectionScope::new(
                SCOPE,
                div()
                    .child(row("a", "one"))
                    .child(row("b", "two"))
                    .child(row("c", "three")),
            ))
        }
    }

    #[gpui::test]
    fn without_a_selection_frame_state_stays_bounded_and_drags_persist(cx: &mut TestAppContext) {
        cx.update(|cx| {
            gpui_component::init(cx);
            crate::Theme::dark().apply(cx);
        });
        let (_, cx) = cx.add_window_view(|window, cx| {
            let view = cx.new(|_| Frameless);
            gpui_component::Root::new(view, window, cx)
        });
        cx.run_until_parked();
        let _ = selected(cx);
        let size = cx.update(|window, cx| state_size(window, cx));
        for _ in 0..5 {
            let _ = selected(cx);
        }
        assert_eq!(cx.update(|window, cx| state_size(window, cx)), size);
        drag(cx, point(px(1.), px(6.)), point(px(30.), px(46.)));
        let _ = selected(cx);
        let text = selected(cx);
        assert!(text.starts_with("one\ntwo\nth"), "{text:?}");
    }

    #[gpui::test]
    fn a_replaced_run_does_not_copy_stale_text(cx: &mut TestAppContext) {
        let age = Rc::new(Cell::new(1));
        let cx = setup_duplicates(age.clone(), true, cx);
        cx.update(|window, cx| select_all_in_scope(&SCOPE.into(), window, cx));
        age.set(2);
        // Before gpui-base sweeps the replaced participant.
        assert_eq!(selected(cx), "same\nsame\nthird\nage 2");
    }

    // A key beside a value that wraps onto several lines.
    struct Wrapped;

    impl Render for Wrapped {
        fn render(&mut self, _: &mut Window, _: &mut Context<Self>) -> impl IntoElement {
            div()
                .size_full()
                .child(SelectionFrame)
                .child(SelectionScope::new(
                    SCOPE,
                    div()
                        .child(
                            div()
                                .flex()
                                .items_start()
                                .gap(px(8.))
                                .child(div().w(px(100.)).child(SelectableLabel::new("k", "Key")))
                                .child(div().w(px(30.)).whitespace_normal().child(
                                    SelectableLabel::new("v", "alphabet bravo charlie delta"),
                                )),
                        )
                        .child(div().h(px(20.)).child(SelectableLabel::new("n", "next"))),
                ))
        }
    }

    #[gpui::test]
    fn a_wrapped_value_stays_on_the_line_of_its_key(cx: &mut TestAppContext) {
        cx.update(|cx| {
            gpui_component::init(cx);
            crate::Theme::dark().apply(cx);
        });
        let (_, cx) = cx.add_window_view(|window, cx| {
            let view = cx.new(|_| Wrapped);
            gpui_component::Root::new(view, window, cx)
        });
        cx.run_until_parked();
        assert!(cx.update(|window, cx| select_all_in_scope(&SCOPE.into(), window, cx)));
        assert_eq!(selected(cx), "Key alphabet bravo charlie delta\nnext");
    }

    // A scroll container under a header, both inside one scope.
    struct HeaderAndScroll(gpui::ScrollHandle);

    impl Render for HeaderAndScroll {
        fn render(&mut self, _: &mut Window, _: &mut Context<Self>) -> impl IntoElement {
            let rows = (0..20u64).map(|n| {
                div()
                    .h(px(20.))
                    .w(px(300.))
                    .child(SelectableLabel::new(("row", n), format!("row {n}")))
            });
            div()
                .size_full()
                .child(SelectionFrame)
                .child(SelectionScope::new(
                    SCOPE,
                    div()
                        .child(
                            div()
                                .h(px(20.))
                                .child(SelectableLabel::new("head", "Header")),
                        )
                        .child(
                            div()
                                .id("scroll")
                                .h(px(100.))
                                .overflow_y_scroll()
                                .track_scroll(&self.0)
                                .children(rows),
                        ),
                ))
        }
    }

    #[gpui::test]
    fn a_drag_past_the_edge_of_a_scroll_container_scrolls_it(cx: &mut TestAppContext) {
        cx.update(|cx| {
            gpui_component::init(cx);
            crate::Theme::dark().apply(cx);
        });
        let handle = gpui::ScrollHandle::new();
        let view_handle = handle.clone();
        let (_, cx) = cx.add_window_view(move |window, cx| {
            let view = cx.new(|_| HeaderAndScroll(view_handle));
            gpui_component::Root::new(view, window, cx)
        });
        cx.run_until_parked();
        let _ = selected(cx);
        let offset = handle.offset();
        // From a row in the container to below it (the container ends at y = 120).
        hold_drag(cx, point(px(2.), px(70.)), point(px(100.), px(160.)));
        assert!(handle.offset().y < offset.y, "{:?}", handle.offset());
        release(cx, point(px(100.), px(160.)));
    }

    #[test]
    fn wrapped_selection_paints_full_width_middle_lines() {
        let bounds = Bounds::new(point(px(10.), px(20.)), gpui::size(px(100.), px(100.)));
        let quads = quad_bounds(
            point(px(40.), px(20.)),
            point(px(30.), px(80.)),
            bounds,
            px(20.),
        );
        assert_eq!(
            quads,
            vec![
                Bounds::from_corners(point(px(40.), px(20.)), point(px(110.), px(40.))),
                Bounds::from_corners(point(px(10.), px(40.)), point(px(110.), px(80.))),
                Bounds::from_corners(point(px(10.), px(80.)), point(px(30.), px(100.))),
            ]
        );
    }
}
