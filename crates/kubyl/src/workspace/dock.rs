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
        if panels.is_empty() {
            panels.push(Box::new(
                cx.new(|cx| PlaceholderView::for_dock(position, cx)),
            ));
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
            active: 0,
            focus: cx.focus_handle(),
            close_action,
        }
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
