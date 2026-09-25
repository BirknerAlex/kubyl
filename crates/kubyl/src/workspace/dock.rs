//! Right and bottom docks: panels contributed through [`kubyl_core::ChromeRegistry`].

use gpui::{
    Action, App, Context, FocusHandle, Focusable, IntoElement, Render, SharedString, Window, div,
    prelude::*,
};
use kubyl_core::{ChromeRegistry, DockPosition, TabHandle};
use kubyl_ui::{ActiveColors, DockHeader, IconButton, IconName, Tab, TabBar, v_flex};

use crate::views::PlaceholderView;

pub struct Dock {
    position: DockPosition,
    panels: Vec<Box<dyn TabHandle>>,
    /// [`kubyl_core::DockPanel::id`] of each entry in `panels` (`None` for the placeholder).
    ids: Vec<Option<&'static str>>,
    active: usize,
    focus: FocusHandle,
    close_action: Box<dyn Action>,
}

impl Focusable for Dock {
    fn focus_handle(&self, _: &App) -> FocusHandle {
        self.focus.clone()
    }
}

impl Dock {
    /// Builds every registered panel for `position`. Shows a placeholder while none exist.
    pub fn new(
        position: DockPosition,
        close_action: Box<dyn Action>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Self {
        let registered: Vec<_> = ChromeRegistry::global(cx)
            .dock_panels(position)
            .cloned()
            .collect();
        let mut panels: Vec<Box<dyn TabHandle>> = registered
            .iter()
            .map(|panel| panel.build(window, cx))
            .collect();
        let mut ids: Vec<Option<&'static str>> =
            registered.iter().map(|panel| Some(panel.id())).collect();
        if panels.is_empty() {
            panels.push(Box::new(
                cx.new(|cx| PlaceholderView::for_dock(position, cx)),
            ));
            ids.push(None);
        }
        for panel in &panels {
            let this = cx.entity().downgrade();
            panel
                .observe(
                    cx,
                    Box::new(move |cx| {
                        this.update(cx, |_, cx| cx.notify()).ok();
                    }),
                )
                .detach();
        }
        Self {
            position,
            panels,
            ids,
            active: 0,
            focus: cx.focus_handle(),
            close_action,
        }
    }

    /// Makes the panel with `id` the active tab. Returns `false` when this dock doesn't hold it.
    pub fn activate_panel(&mut self, id: &str, cx: &mut Context<Self>) -> bool {
        let Some(index) = self.ids.iter().position(|i| *i == Some(id)) else {
            return false;
        };
        self.active = index;
        cx.notify();
        true
    }

    /// Focus handle of the active panel.
    pub fn active_focus_handle(&self, cx: &App) -> FocusHandle {
        self.panels
            .get(self.active)
            .map(|panel| panel.focus_handle(cx))
            .unwrap_or_else(|| self.focus.clone())
    }

    fn close_button(&self) -> IconButton {
        let action = self.close_action.boxed_clone();
        IconButton::new("close-dock", IconName::X)
            .icon_size(13.0)
            .on_click(move |_, window, cx| window.dispatch_action(action.boxed_clone(), cx))
    }
}

impl Render for Dock {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let colors = cx.colors().clone();
        let header = if self.panels.len() > 1 {
            let tabs = self.panels.iter().enumerate().map(|(index, panel)| {
                Tab::new(
                    SharedString::from(format!("dock-tab-{index}")),
                    panel.title(cx),
                )
                .icon(panel.icon(cx))
                .active(index == self.active)
                .on_click(cx.listener(move |this, _, _, cx| {
                    this.active = index;
                    cx.notify();
                }))
            });
            TabBar::new("dock-tabs")
                .tabs(tabs)
                .tool(self.close_button())
                .into_any_element()
        } else {
            let title = self.panels.first().map(|p| p.title(cx)).unwrap_or_default();
            DockHeader::new(title)
                .end_child(self.close_button())
                .into_any_element()
        };
        let content = self
            .panels
            .get(self.active)
            .map(|panel| panel.to_any_view());
        v_flex()
            .id(match self.position {
                DockPosition::Left => "left-dock",
                DockPosition::Right => "right-dock",
                DockPosition::Bottom => "bottom-dock",
            })
            .track_focus(&self.focus)
            .size_full()
            .bg(colors.panel)
            .map(|this| match self.position {
                DockPosition::Right => this.border_l_1().border_color(colors.border),
                DockPosition::Bottom => this.border_t_1().border_color(colors.border),
                DockPosition::Left => this.border_r_1().border_color(colors.border),
            })
            .child(header)
            .child(div().flex_1().min_h_0().overflow_hidden().children(content))
    }
}
