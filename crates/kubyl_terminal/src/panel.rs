//! The Terminal panel of the bottom dock (board 2): terminal tabs, each split into side-by-side
//! terminals. Shells, attaches, debug containers and node shells open here; `alt-s` opens a
//! shell as an editor tab instead.

use std::collections::HashMap;

use gpui::{
    AnyWindowHandle, App, AppContext as _, Context, Entity, EntityId, FocusHandle, Focusable,
    Global, IntoElement, SharedString, Subscription, WeakEntity, Window, WindowId, div, prelude::*,
};
use kubyl_core::actions::ActivateDockPanel;
use kubyl_core::{DockPanel, DockPosition, TabHandle, TabView};
use kubyl_ui::{ActiveColors, IconButton, IconName, Tab, TabBar, h_flex, u, v_flex};

use crate::view::{SessionMode, TerminalSpec, TerminalView};

/// [`DockPanel::id`] of the panel, for [`ActivateDockPanel`].
pub const PANEL_ID: &str = "terminal";

/// The panel of each window.
#[derive(Default)]
struct Panels(HashMap<WindowId, WeakEntity<TerminalPanel>>);

impl Global for Panels {}

pub struct TerminalDockPanel;

impl DockPanel for TerminalDockPanel {
    fn id(&self) -> &'static str {
        PANEL_ID
    }

    fn position(&self) -> DockPosition {
        DockPosition::Bottom
    }

    fn build(&self, window: &mut Window, cx: &mut App) -> Box<dyn TabHandle> {
        let panel = cx.new(TerminalPanel::new);
        let id = window.window_handle().window_id();
        cx.default_global::<Panels>()
            .0
            .insert(id, panel.downgrade());
        Box::new(panel)
    }
}

/// The panel of `window`, if the window has one.
pub fn panel_for(window: AnyWindowHandle, cx: &App) -> Option<Entity<TerminalPanel>> {
    cx.try_global::<Panels>()?
        .0
        .get(&window.window_id())?
        .upgrade()
}

/// Opens `spec` in a new tab of the window's Terminal panel and shows the panel. Returns
/// `false` when the window has no panel.
pub fn open_in_panel(spec: TerminalSpec, window: &mut Window, cx: &mut App) -> bool {
    let Some(panel) = panel_for(window.window_handle(), cx) else {
        return false;
    };
    panel.update(cx, |panel, cx| panel.open(spec, window, cx));
    window.dispatch_action(Box::new(ActivateDockPanel(PANEL_ID.into())), cx);
    true
}

struct PanelTab {
    terminals: Vec<Entity<TerminalView>>,
    active: usize,
}

pub struct TerminalPanel {
    focus: FocusHandle,
    tabs: Vec<PanelTab>,
    active: usize,
    subscriptions: HashMap<EntityId, Subscription>,
}

impl TerminalPanel {
    fn new(cx: &mut Context<Self>) -> Self {
        Self {
            focus: cx.focus_handle(),
            tabs: Vec::new(),
            active: 0,
            subscriptions: HashMap::new(),
        }
    }

    fn terminal(&mut self, spec: TerminalSpec, cx: &mut Context<Self>) -> Entity<TerminalView> {
        let terminal = cx.new(|cx| TerminalView::new(spec, false, cx));
        self.subscriptions.insert(
            terminal.entity_id(),
            cx.observe(&terminal, |_, _, cx| cx.notify()),
        );
        terminal
    }

    /// Opens a terminal in a new tab and focuses it.
    pub fn open(&mut self, spec: TerminalSpec, window: &mut Window, cx: &mut Context<Self>) {
        let terminal = self.terminal(spec, cx);
        self.tabs.push(PanelTab {
            terminals: vec![terminal.clone()],
            active: 0,
        });
        self.active = self.tabs.len() - 1;
        terminal.read(cx).focus_handle(cx).focus(window, cx);
        cx.notify();
    }

    fn active_terminal(&self) -> Option<&Entity<TerminalView>> {
        let tab = self.tabs.get(self.active)?;
        tab.terminals.get(tab.active)
    }

    /// A new exec shell for the active terminal's pod: in a new tab, or next to it (split).
    fn new_like_active(&mut self, split: bool, window: &mut Window, cx: &mut Context<Self>) {
        let Some(active) = self.active_terminal() else {
            return;
        };
        let current = active.read(cx).spec().clone();
        let spec = match current.mode {
            // A node shell's pod belongs to its session: `+` starts another node shell.
            SessionMode::NodeShell { .. } => current,
            _ => TerminalSpec {
                mode: SessionMode::Exec { shell: None },
                ..current
            },
        };
        if split {
            let terminal = self.terminal(spec, cx);
            let tab = &mut self.tabs[self.active];
            tab.terminals.push(terminal.clone());
            tab.active = tab.terminals.len() - 1;
            terminal.read(cx).focus_handle(cx).focus(window, cx);
            cx.notify();
        } else {
            self.open(spec, window, cx);
        }
    }

    fn close_tab(&mut self, index: usize, window: &mut Window, cx: &mut Context<Self>) {
        if index >= self.tabs.len() {
            return;
        }
        let tab = self.tabs.remove(index);
        for terminal in tab.terminals {
            self.subscriptions.remove(&terminal.entity_id());
        }
        if self.active >= self.tabs.len() {
            self.active = self.tabs.len().saturating_sub(1);
        }
        match self.active_terminal() {
            Some(terminal) => terminal.read(cx).focus_handle(cx).focus(window, cx),
            None => self.focus.focus(window, cx),
        }
        cx.notify();
    }

    fn close_split(&mut self, index: usize, window: &mut Window, cx: &mut Context<Self>) {
        let Some(tab) = self.tabs.get_mut(self.active) else {
            return;
        };
        if tab.terminals.len() <= 1 {
            let active = self.active;
            self.close_tab(active, window, cx);
            return;
        }
        let terminal = tab.terminals.remove(index);
        self.subscriptions.remove(&terminal.entity_id());
        tab.active = tab.active.min(tab.terminals.len() - 1);
        cx.notify();
    }

    fn activate(&mut self, index: usize, window: &mut Window, cx: &mut Context<Self>) {
        self.active = index;
        if let Some(terminal) = self.active_terminal() {
            terminal.read(cx).focus_handle(cx).focus(window, cx);
        }
        cx.notify();
    }
}

impl Focusable for TerminalPanel {
    fn focus_handle(&self, cx: &App) -> FocusHandle {
        match self.active_terminal() {
            Some(terminal) => terminal.read(cx).focus_handle(cx),
            None => self.focus.clone(),
        }
    }
}

impl TabView for TerminalPanel {
    fn tab_title(&self, _: &App) -> SharedString {
        "Terminal".into()
    }

    fn tab_icon(&self, _: &App) -> Option<SharedString> {
        Some(IconName::Terminal.path())
    }
}

impl Render for TerminalPanel {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let colors = cx.colors().clone();
        if self.tabs.is_empty() {
            return v_flex()
                .track_focus(&self.focus)
                .size_full()
                .items_center()
                .justify_center()
                .gap(u(6.0))
                .text_color(colors.text_dim)
                .child("No terminals.")
                .child(
                    div()
                        .text_size(u(12.0))
                        .text_color(colors.text_faint)
                        .child("Select a pod and press s for a shell, a to attach."),
                )
                .into_any_element();
        }
        let weak = cx.weak_entity();
        let tabs = self.tabs.iter().enumerate().map(|(index, tab)| {
            let close = weak.clone();
            let title = tab
                .terminals
                .first()
                .map(|t| t.read(cx).title())
                .unwrap_or_default();
            let title = if tab.terminals.len() > 1 {
                format!("{title} +{}", tab.terminals.len() - 1)
            } else {
                title
            };
            Tab::new(SharedString::from(format!("terminal-tab-{index}")), title)
                .icon(Some(IconName::Terminal.path()))
                .active(index == self.active)
                .on_click(cx.listener(move |this, _, window, cx| this.activate(index, window, cx)))
                .on_close(move |window, cx| {
                    close
                        .update(cx, |this, cx| this.close_tab(index, window, cx))
                        .ok();
                })
        });
        let bar = TabBar::new("terminal-tabs")
            .tabs(tabs)
            .tool(
                IconButton::new("terminal-new", IconName::Plus)
                    .icon_size(14.0)
                    .on_click(
                        cx.listener(|this, _, window, cx| this.new_like_active(false, window, cx)),
                    ),
            )
            .tool(
                IconButton::new("terminal-split", IconName::Columns)
                    .icon_size(13.0)
                    .on_click(
                        cx.listener(|this, _, window, cx| this.new_like_active(true, window, cx)),
                    ),
            );
        let tab = &self.tabs[self.active];
        let split = tab.terminals.len() > 1;
        let terminals = tab.terminals.iter().enumerate().map(|(index, terminal)| {
            div()
                .relative()
                .flex_1()
                .min_w_0()
                .h_full()
                .when(index > 0, |this| {
                    this.border_l_1().border_color(colors.border)
                })
                .child(terminal.clone())
                .when(split, |this| {
                    this.child(
                        div().absolute().top(u(1.0)).right(u(2.0)).child(
                            IconButton::new(("terminal-close-split", index), IconName::X)
                                .icon_size(11.0)
                                .on_click(cx.listener(move |this, _, window, cx| {
                                    this.close_split(index, window, cx)
                                })),
                        ),
                    )
                })
        });
        v_flex()
            .track_focus(&self.focus)
            .size_full()
            .child(bar)
            .child(
                h_flex()
                    .flex_1()
                    .min_h_0()
                    .items_start()
                    .children(terminals),
            )
            .into_any_element()
    }
}
