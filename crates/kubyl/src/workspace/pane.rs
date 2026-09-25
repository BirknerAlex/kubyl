//! A pane: a tab bar and the active tab's view.

use std::collections::HashMap;

use gpui::{
    App, Context, EntityId, EventEmitter, FocusHandle, Focusable, IntoElement, Render,
    SharedString, Subscription, Window, actions, div, prelude::*,
};
use kubyl_core::{TabHandle, ViewKind, ViewRequest};
use kubyl_ui::{ActiveColors, IconButton, IconName, Tab, TabBar, u, v_flex};

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
            _subscriptions: vec![focus_sub],
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
        let this = cx.entity().downgrade();
        let subscription = item.observe(
            cx,
            Box::new(move |cx| {
                this.update(cx, |_, cx| cx.notify()).ok();
            }),
        );
        self.item_subscriptions
            .insert(item.entity_id(), subscription);
        let index = if self.items.is_empty() {
            0
        } else {
            self.active + 1
        };
        self.items.insert(index, item);
        self.activate(index, focus, window, cx);
        cx.emit(PaneEvent::Changed);
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
                tabs.push(request);
            }
        }
        PaneLayout::Pane { tabs, active }
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

    fn render_tab_bar(&self, cx: &mut Context<Self>) -> TabBar {
        let tabs = self.items.iter().enumerate().map(|(index, item)| {
            let this = cx.entity().downgrade();
            Tab::new(
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
        });
        TabBar::new("tabs")
            .tabs(tabs)
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
