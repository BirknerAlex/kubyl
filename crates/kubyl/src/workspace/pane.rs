//! A pane: a tab bar and the active tab's view.

use std::collections::HashMap;

use gpui::{
    App, Context, EntityId, EventEmitter, FocusHandle, Focusable, IntoElement, Render,
    SharedString, Subscription, WeakEntity, Window, actions, div, prelude::*,
};
use gpui_component::menu::{ContextMenuExt as _, PopupMenuItem};
use kubyl_core::{ClusterIds, TabHandle, ViewKind, ViewRequest};
use kubyl_explorer::favorites::{self, Favorites};
use kubyl_ui::{ActiveColors, Icon, IconButton, IconName, Tab, TabBar, h_flex, u, v_flex};

use super::layout::{PaneLayout, SplitAxis};

actions!(
    pane,
    [
        /// Closes the active tab.
        CloseActiveTab,
        ActivateNextTab,
        ActivatePreviousTab,
        /// Splits the pane; the new pane appears on the right.
        SplitRight,
        /// Splits the pane; the new pane appears below.
        SplitDown,
        /// Fills the window with the active pane (docks hidden).
        ToggleZoom,
        /// Adds the active tab's view to the favorites, or removes it.
        ToggleFavoriteView,
        /// Opens a new tab.
        NewTab,
        /// Goes back to the previously active tab of this pane (reopens it if it was closed).
        GoBack,
        /// Undoes [`GoBack`].
        GoForward,
    ]
);

/// Most tabs a pane's history remembers.
const HISTORY_LIMIT: usize = 100;

pub enum PaneEvent {
    /// The pane or one of its views got focus.
    Focused,
    Split(SplitAxis),
    /// The last tab was closed.
    Emptied,
    ToggleZoom,
    /// Tabs were opened, closed or reordered.
    Changed,
}

/// A tab being dragged to another position or pane.
pub struct DraggedTab {
    pub(super) pane: WeakEntity<Pane>,
    pub(super) item: EntityId,
    title: SharedString,
    icon: Option<SharedString>,
}

impl DraggedTab {
    #[cfg(test)]
    pub(super) fn new(pane: WeakEntity<Pane>, item: EntityId) -> Self {
        Self {
            pane,
            item,
            title: SharedString::default(),
            icon: None,
        }
    }
}

struct TabDragPreview {
    title: SharedString,
    icon: Option<SharedString>,
}

impl Render for TabDragPreview {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let colors = cx.colors();
        h_flex()
            .px(u(10.0))
            .py(u(4.0))
            .gap(u(7.0))
            .rounded(u(4.0))
            .bg(colors.elevated)
            .border_1()
            .border_color(colors.accent)
            .text_size(u(12.5))
            .text_color(colors.text)
            .when_some(self.icon.clone(), |this, path| {
                this.child(Icon::from_path(path).size(13.0).color(colors.accent))
            })
            .child(self.title.clone())
    }
}

pub struct Pane {
    items: Vec<Box<dyn TabHandle>>,
    active: usize,
    focus: FocusHandle,
    zoomed: bool,
    /// Requests of the tabs activated in this pane, oldest first (`⌘[` / `⌘]`).
    history: Vec<ViewRequest>,
    /// Position of the active tab in `history`.
    history_pos: usize,
    /// Set while going back or forward, so that activation doesn't record history.
    navigating: bool,
    /// A close of tabs that asked for it ([`kubyl_core::TabView::wants_close`]) is scheduled.
    closing_wanted: bool,
    item_subscriptions: HashMap<EntityId, Subscription>,
    _subscriptions: Vec<Subscription>,
}

impl EventEmitter<PaneEvent> for Pane {}

impl Focusable for Pane {
    fn focus_handle(&self, _: &App) -> FocusHandle {
        self.focus.clone()
    }
}

impl Pane {
    pub fn new(window: &mut Window, cx: &mut Context<Self>) -> Self {
        let focus = cx.focus_handle();
        let focus_sub = cx.on_focus_in(&focus, window, |_, _, cx| cx.emit(PaneEvent::Focused));
        // The star in the tab bar follows the favorites.
        let favorites_sub = Favorites::try_global(cx)
            .map(|favorites| cx.observe(&favorites, |_, _, cx| cx.notify()));
        Self {
            items: Vec::new(),
            active: 0,
            focus,
            zoomed: false,
            history: Vec::new(),
            history_pos: 0,
            navigating: false,
            closing_wanted: false,
            item_subscriptions: HashMap::new(),
            _subscriptions: [Some(focus_sub), favorites_sub]
                .into_iter()
                .flatten()
                .collect(),
        }
    }

    #[cfg(test)]
    pub fn items(&self) -> &[Box<dyn TabHandle>] {
        &self.items
    }

    pub fn active_item(&self) -> Option<&dyn TabHandle> {
        self.items.get(self.active).map(|item| item.as_ref())
    }

    pub fn set_zoomed(&mut self, zoomed: bool, cx: &mut Context<Self>) {
        self.zoomed = zoomed;
        cx.notify();
    }

    /// Adds `item` after the active tab and activates it. If a tab for the same request is
    /// already open, activates that one instead and drops `item`.
    pub fn add_item(
        &mut self,
        item: Box<dyn TabHandle>,
        focus: bool,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if let Some(request) = item.view_request(cx)
            && let Some(existing) = self
                .items
                .iter()
                .position(|i| i.view_request(cx).as_ref() == Some(&request))
        {
            self.activate(existing, focus, window, cx);
            return;
        }
        let index = if self.items.is_empty() {
            0
        } else {
            self.active + 1
        };
        self.insert_item(index, item, focus, window, cx);
    }

    /// Inserts `item` at `index` (clamped) and activates it.
    fn insert_item(
        &mut self,
        index: usize,
        item: Box<dyn TabHandle>,
        focus: bool,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.observe_item(item.as_ref(), cx);
        let index = index.min(self.items.len());
        self.items.insert(index, item);
        self.activate(index, focus, window, cx);
        cx.emit(PaneEvent::Changed);
    }

    fn observe_item(&mut self, item: &dyn TabHandle, cx: &mut Context<Self>) {
        let this = cx.entity().downgrade();
        let subscription = item.observe(
            cx,
            Box::new(move |cx| {
                this.update(cx, |_, cx| cx.notify()).ok();
            }),
        );
        self.item_subscriptions
            .insert(item.entity_id(), subscription);
    }

    /// Removes the tab showing `item` without closing its view, for moving it to another pane.
    /// The pane reports [`PaneEvent::Emptied`] when that was its last tab.
    fn take_item(
        &mut self,
        item: EntityId,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Option<Box<dyn TabHandle>> {
        let index = self.items.iter().position(|i| i.entity_id() == item)?;
        let taken = self.items.remove(index);
        self.item_subscriptions.remove(&item);
        if self.items.is_empty() {
            self.active = 0;
            cx.emit(PaneEvent::Emptied);
        } else {
            if self.active > index || self.active == self.items.len() {
                self.active -= 1;
            }
            let active = self.active;
            self.activate(active, false, window, cx);
        }
        cx.emit(PaneEvent::Changed);
        cx.notify();
        Some(taken)
    }

    /// Moves the dragged tab to `index` (the end when `None`) of this pane and activates it.
    pub(super) fn drop_tab(
        &mut self,
        dragged: &DraggedTab,
        index: Option<usize>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let this = cx.entity().downgrade();
        if dragged.pane == this {
            let Some(from) = self
                .items
                .iter()
                .position(|i| i.entity_id() == dragged.item)
            else {
                return;
            };
            let item = self.items.remove(from);
            let to = index.unwrap_or(self.items.len()).min(self.items.len());
            self.items.insert(to, item);
            self.activate(to, true, window, cx);
            cx.emit(PaneEvent::Changed);
            cx.notify();
            return;
        }
        let Some(source) = dragged.pane.upgrade() else {
            return;
        };
        let Some((request, dirty)) = source
            .read(cx)
            .items
            .iter()
            .find(|i| i.entity_id() == dragged.item)
            .map(|i| (i.view_request(cx), i.is_dirty(cx)))
        else {
            return;
        };
        // This pane already shows that view: keep one tab, at the drop position. The one with
        // unsaved edits stays; if both have some, nothing moves.
        let existing = request.as_ref().and_then(|request| {
            self.items
                .iter()
                .position(|i| i.view_request(cx).as_ref() == Some(request))
        });
        if let Some(existing) = existing
            && dirty
            && self.items[existing].is_dirty(cx)
        {
            return;
        }
        let item = dragged.item;
        let Some(item) = source.update(cx, |pane, cx| pane.take_item(item, window, cx)) else {
            return;
        };
        let mut index = index.unwrap_or(self.items.len());
        let item = match existing {
            Some(existing) => {
                let kept = self.items.remove(existing);
                self.item_subscriptions.remove(&kept.entity_id());
                if existing < index {
                    index -= 1;
                }
                if dirty { item } else { kept }
            }
            None => item,
        };
        self.insert_item(index, item, false, window, cx);
        // After the workspace handled the source pane emptying (it focuses another pane).
        cx.defer_in(window, |this, window, cx| {
            this.active_focus_handle(cx).focus(window, cx);
        });
    }

    pub fn activate(
        &mut self,
        index: usize,
        focus: bool,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if index >= self.items.len() {
            return;
        }
        if self.active != index {
            self.active = index;
            cx.emit(PaneEvent::Changed);
        }
        self.record_history(cx);
        if focus {
            self.items[index].focus_handle(cx).focus(window, cx);
        }
        cx.notify();
    }

    pub fn close(&mut self, index: usize, window: &mut Window, cx: &mut Context<Self>) {
        if index >= self.items.len() {
            return;
        }
        let item = self.items.remove(index);
        self.item_subscriptions.remove(&item.entity_id());
        if self.items.is_empty() {
            self.active = 0;
            self.focus.focus(window, cx);
            cx.emit(PaneEvent::Emptied);
        } else {
            if self.active >= index && self.active > 0 {
                self.active -= 1;
            }
            let active = self.active;
            self.activate(active, true, window, cx);
        }
        cx.emit(PaneEvent::Changed);
        cx.notify();
    }

    /// Remembers the active tab's request, dropping the forward part of the history.
    fn record_history(&mut self, cx: &App) {
        if self.navigating {
            return;
        }
        let Some(request) = self.active_item().and_then(|item| item.view_request(cx)) else {
            return;
        };
        if self.history.get(self.history_pos) == Some(&request) {
            return;
        }
        self.history.truncate(self.history_pos + 1);
        self.history.push(request);
        if self.history.len() > HISTORY_LIMIT {
            self.history.remove(0);
        }
        self.history_pos = self.history.len() - 1;
    }

    /// Moves `delta` steps through the history and shows that tab, reopening it if it was
    /// closed. Returns false at either end.
    pub fn navigate(&mut self, delta: isize, window: &mut Window, cx: &mut Context<Self>) -> bool {
        let Some(pos) = self.history_pos.checked_add_signed(delta) else {
            return false;
        };
        let Some(request) = self.history.get(pos).cloned() else {
            return false;
        };
        self.history_pos = pos;
        self.navigating = true;
        match self
            .items
            .iter()
            .position(|i| i.view_request(cx).as_ref() == Some(&request))
        {
            Some(index) => self.activate(index, true, window, cx),
            None => {
                let item = super::build_view(&request, window, cx);
                self.add_item(item, true, window, cx);
            }
        }
        self.navigating = false;
        true
    }

    /// The focus handle of the active tab, or the pane's own when it is empty.
    pub fn active_focus_handle(&self, cx: &App) -> FocusHandle {
        match self.items.get(self.active) {
            Some(item) => item.focus_handle(cx),
            None => self.focus.clone(),
        }
    }

    pub fn layout(&self, cx: &App) -> PaneLayout {
        let mut active = 0;
        let mut tabs = Vec::new();
        for (index, item) in self.items.iter().enumerate() {
            if let Some(request) = item.view_request(cx) {
                if index == self.active {
                    active = tabs.len();
                }
                // Saved with current cluster ids (see `ClusterIds`).
                tabs.push(ClusterIds::normalize(cx, &request));
            }
        }
        PaneLayout::Pane { tabs, active }
    }

    /// Rebuilds tabs whose cluster id resolves to another one now (contexts were grouped, an
    /// entry was re-keyed), in place.
    pub fn refresh_clusters(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let had_focus = self.focus.contains_focused(window, cx);
        let mut changed = false;
        for index in 0..self.items.len() {
            // Unsaved edits stay: the manager still resolves the old id, and the saved layout
            // uses the current one.
            if self.items[index].is_dirty(cx) {
                continue;
            }
            let Some(request) = self.items[index].view_request(cx) else {
                continue;
            };
            let current = ClusterIds::normalize(cx, &request);
            if current == request {
                continue;
            }
            let item = super::build_view(&current, window, cx);
            let this = cx.entity().downgrade();
            let subscription = item.observe(
                cx,
                Box::new(move |cx| {
                    this.update(cx, |_, cx| cx.notify()).ok();
                }),
            );
            let old = std::mem::replace(&mut self.items[index], item);
            self.item_subscriptions.remove(&old.entity_id());
            self.item_subscriptions
                .insert(self.items[index].entity_id(), subscription);
            changed = true;
        }
        if !changed {
            return;
        }
        for request in &mut self.history {
            *request = ClusterIds::normalize(cx, request);
        }
        if had_focus {
            let focus = self.active_focus_handle(cx);
            focus.focus(window, cx);
        }
        cx.emit(PaneEvent::Changed);
        cx.notify();
    }

    /// Closes the tabs whose views asked for it.
    fn close_wanted(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.closing_wanted = false;
        while let Some(index) = self.items.iter().position(|item| item.wants_close(cx)) {
            self.close(index, window, cx);
        }
    }

    fn close_active_tab(
        &mut self,
        _: &CloseActiveTab,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if self.items.is_empty() {
            // Nothing to close: let the window handle it (closes an empty split).
            cx.emit(PaneEvent::Emptied);
            return;
        }
        self.close(self.active, window, cx);
    }

    fn activate_next(&mut self, _: &ActivateNextTab, window: &mut Window, cx: &mut Context<Self>) {
        if !self.items.is_empty() {
            let next = (self.active + 1) % self.items.len();
            self.activate(next, true, window, cx);
        }
    }

    fn activate_previous(
        &mut self,
        _: &ActivatePreviousTab,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if !self.items.is_empty() {
            let len = self.items.len();
            self.activate((self.active + len - 1) % len, true, window, cx);
        }
    }

    fn new_tab(&mut self, _: &NewTab, window: &mut Window, cx: &mut Context<Self>) {
        let item = super::build_view(&ViewRequest::new(ViewKind::Welcome), window, cx);
        self.add_item(item, true, window, cx);
    }

    fn toggle_favorite_view(
        &mut self,
        _: &ToggleFavoriteView,
        _: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if let Some(item) = self.items.get(self.active) {
            toggle_favorite(item.as_ref(), cx);
        }
    }

    fn render_tab_bar(&self, cx: &mut Context<Self>) -> TabBar {
        let drop_background = cx.colors().accent.opacity(0.12);
        let tabs = self.items.iter().enumerate().map(|(index, item)| {
            let this = cx.entity().downgrade();
            let dragged = DraggedTab {
                pane: this.clone(),
                item: item.entity_id(),
                title: item.title(cx),
                icon: item.icon(cx),
            };
            let on_drop = cx.listener(move |this, dragged: &DraggedTab, window, cx| {
                this.drop_tab(dragged, Some(index), window, cx)
            });
            let tab_item = item.boxed_clone();
            let tab = Tab::new(
                SharedString::from(format!("tab-{}", item.entity_id())),
                item.title(cx),
            )
            .icon(item.icon(cx))
            .dot(item.dot(cx))
            .active(index == self.active)
            .dirty(item.is_dirty(cx))
            .on_click(
                cx.listener(move |this, _, window, cx| this.activate(index, true, window, cx)),
            )
            .on_close(move |window, cx| {
                this.update(cx, |this, cx| this.close(index, window, cx))
                    .ok();
            })
            .decorate(move |tab| {
                tab.on_drag(dragged, |dragged, _, _, cx| {
                    cx.new(|_| TabDragPreview {
                        title: dragged.title.clone(),
                        icon: dragged.icon.clone(),
                    })
                })
                .drag_over::<DraggedTab>(move |style, _, _, _| style.bg(drop_background))
                .on_drop(on_drop)
            });
            h_flex()
                .id(SharedString::from(format!("tab-menu-{}", item.entity_id())))
                .flex_none()
                .h_full()
                .child(tab)
                .context_menu(move |menu, _, cx| {
                    let is_favorite = favorite_state(tab_item.as_ref(), cx);
                    let item = tab_item.boxed_clone();
                    menu.item(
                        PopupMenuItem::new(match is_favorite {
                            Some(true) => "Remove from Favorites",
                            _ => "Add to Favorites",
                        })
                        .disabled(is_favorite.is_none())
                        .on_click(move |_, _, cx| toggle_favorite(item.as_ref(), cx)),
                    )
                })
        });
        let on_drop_end = cx.listener(|this, dragged: &DraggedTab, window, cx| {
            this.drop_tab(dragged, None, window, cx)
        });
        // The star of the active tab's view (only for views that can be reopened).
        let favorite_state = self
            .items
            .get(self.active)
            .and_then(|item| favorite_state(item.as_ref(), cx));
        let favorite_button = favorite_state.map(|is_favorite| {
            IconButton::new(
                "favorite-view",
                if is_favorite {
                    IconName::StarFilled
                } else {
                    IconName::Star
                },
            )
            .icon_size(13.0)
            .toggled(is_favorite)
            .on_click(|_, window, cx| window.dispatch_action(Box::new(ToggleFavoriteView), cx))
        });
        let mut bar = TabBar::new("tabs");
        for tab in tabs {
            bar = bar.tab_element(tab);
        }
        if let Some(button) = favorite_button {
            bar = bar.tool(button);
        }
        bar.decorate_end(move |end| {
            end.drag_over::<DraggedTab>(move |style, _, _, _| style.bg(drop_background))
                .on_drop(on_drop_end)
        })
        .tool(
            IconButton::new("new-tab", IconName::Plus)
                .on_click(|_, window, cx| window.dispatch_action(Box::new(NewTab), cx)),
        )
        .tool(
            IconButton::new("split", IconName::Columns)
                .on_click(|_, window, cx| window.dispatch_action(Box::new(SplitRight), cx)),
        )
        .tool(
            IconButton::new(
                "zoom",
                if self.zoomed {
                    IconName::Minimize
                } else {
                    IconName::Maximize
                },
            )
            .icon_size(13.0)
            .toggled(self.zoomed)
            .on_click(|_, window, cx| window.dispatch_action(Box::new(ToggleZoom), cx)),
        )
    }
}

/// Whether the view of `item` is a favorite (`None`: it can't be one).
fn favorite_state(item: &dyn TabHandle, cx: &App) -> Option<bool> {
    let favorite = favorites::from_tab(item, cx)?;
    let saved = Favorites::try_global(cx)?
        .read(cx)
        .position(&favorite, cx)
        .is_some();
    Some(saved)
}

/// Adds the view of `item` to the favorites, or removes it.
fn toggle_favorite(item: &dyn TabHandle, cx: &mut App) {
    let (Some(favorite), Some(favorites)) =
        (favorites::from_tab(item, cx), Favorites::try_global(cx))
    else {
        return;
    };
    favorites.update(cx, |favorites, cx| favorites.toggle(favorite, cx));
}

impl Render for Pane {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        // Items notify the pane when they change; one may have asked to be closed.
        if !self.closing_wanted && self.items.iter().any(|item| item.wants_close(cx)) {
            self.closing_wanted = true;
            cx.defer_in(window, |this, window, cx| this.close_wanted(window, cx));
        }
        let colors = cx.colors().clone();
        let content = match self.items.get(self.active) {
            Some(item) => div()
                .size_full()
                .child(item.to_any_view())
                .into_any_element(),
            None => v_flex()
                .size_full()
                .items_center()
                .justify_center()
                .gap(u(6.0))
                .text_color(colors.text_dim)
                .child("No open tabs")
                .child(
                    div()
                        .text_size(u(12.0))
                        .text_color(colors.text_faint)
                        .child("Open a resource from the sidebar, or press + for a new tab."),
                )
                .into_any_element(),
        };
        v_flex()
            .key_context("Pane")
            .track_focus(&self.focus)
            .size_full()
            .min_w_0()
            .bg(colors.background)
            .on_action(cx.listener(Self::close_active_tab))
            .on_action(cx.listener(Self::activate_next))
            .on_action(cx.listener(Self::activate_previous))
            .on_action(cx.listener(Self::new_tab))
            .on_action(cx.listener(Self::toggle_favorite_view))
            .on_action(cx.listener(|this, _: &GoBack, window, cx| {
                this.navigate(-1, window, cx);
            }))
            .on_action(cx.listener(|this, _: &GoForward, window, cx| {
                this.navigate(1, window, cx);
            }))
            .on_action(cx.listener(|_, _: &SplitRight, _, cx| {
                cx.emit(PaneEvent::Split(SplitAxis::Horizontal))
            }))
            .on_action(
                cx.listener(|_, _: &SplitDown, _, cx| {
                    cx.emit(PaneEvent::Split(SplitAxis::Vertical))
                }),
            )
            .on_action(cx.listener(|_, _: &ToggleZoom, _, cx| cx.emit(PaneEvent::ToggleZoom)))
            .child(self.render_tab_bar(cx))
            .child(div().flex_1().min_h_0().overflow_hidden().child(content))
    }
}
