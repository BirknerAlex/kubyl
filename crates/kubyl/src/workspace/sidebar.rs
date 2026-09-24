//! The left sidebar: the Explorer header and the registered sidebar sections.

use gpui::{
    AnyView, App, Context, FocusHandle, Focusable, IntoElement, Render, Window, div, prelude::*,
};
use kubyl_core::ChromeRegistry;
use kubyl_core::actions::{AddKubeconfig, FilterSidebar};
use kubyl_ui::{ActiveColors, IconButton, IconName, PanelHeader, v_flex};

pub struct Sidebar {
    sections: Vec<AnyView>,
    focus: FocusHandle,
}

impl Focusable for Sidebar {
    fn focus_handle(&self, _: &App) -> FocusHandle {
        self.focus.clone()
    }
}

impl Sidebar {
    pub fn new(window: &mut Window, cx: &mut Context<Self>) -> Self {
        let registered: Vec<_> = ChromeRegistry::global(cx).sidebar_sections().to_vec();
        let sections: Vec<AnyView> = registered
            .iter()
            .map(|section| section.build(window, cx))
            .collect();
        Self {
            sections,
            focus: cx.focus_handle(),
        }
    }
}

impl Render for Sidebar {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let colors = cx.colors();
        v_flex()
            .id("sidebar")
            .track_focus(&self.focus)
            .size_full()
            .bg(colors.panel)
            .border_r_1()
            .border_color(colors.border)
            .child(
                PanelHeader::new("Explorer")
                    .end_child(
                        IconButton::new("filter-kinds", IconName::Search)
                            .icon_size(13.0)
                            .on_click(|_, window, cx| {
                                crate::views::dispatch_or_explain(
                                    Box::new(FilterSidebar),
                                    "Filtering the sidebar",
                                    window,
                                    cx,
                                )
                            }),
                    )
                    .end_child(IconButton::new("add-kubeconfig", IconName::Plus).on_click(
                        |_, window, cx| {
                            crate::views::dispatch_or_explain(
                                Box::new(AddKubeconfig),
                                "Adding kubeconfigs",
                                window,
                                cx,
                            )
                        },
                    ))
                    .end_child(IconButton::new("sidebar-more", IconName::Ellipsis)),
            )
            .child(
                div()
                    .id("sidebar-sections")
                    .flex_1()
                    .min_h_0()
                    .overflow_y_scroll()
                    .children(self.sections.iter().cloned()),
            )
    }
}
