//! The left sidebar: the Explorer header and the registered sidebar sections.

use gpui::{
    AnyView, App, Context, FocusHandle, Focusable, IntoElement, Render, Window, div, prelude::*,
};
use gpui_component::button::{Button as MenuButton, ButtonVariants as _};
use gpui_component::menu::{DropdownMenu as _, PopupMenuItem};
use kubyl_core::ChromeRegistry;
use kubyl_core::actions::{AddKubeconfig, FilterSidebar, NewKubeconfig};
use kubyl_kube::ui::{OpenClusters, PasteKubeconfig};
use kubyl_ui::{ActiveColors, Icon, IconButton, IconName, PanelHeader, v_flex};

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
                                window.dispatch_action(Box::new(FilterSidebar), cx)
                            }),
                    )
                    .end_child(
                        MenuButton::new("add-kubeconfig")
                            .ghost()
                            .compact()
                            .child(Icon::new(IconName::Plus).size(14.0))
                            .dropdown_menu(|menu, _, _| {
                                menu.item(PopupMenuItem::new("New kubeconfig…").on_click(
                                    |_, window, cx| {
                                        window.dispatch_action(Box::new(NewKubeconfig), cx)
                                    },
                                ))
                                .item(
                                    PopupMenuItem::new("Add kubeconfig file or folder…").on_click(
                                        |_, window, cx| {
                                            window.dispatch_action(Box::new(AddKubeconfig), cx)
                                        },
                                    ),
                                )
                                .item(PopupMenuItem::new("Paste kubeconfig YAML…").on_click(
                                    |_, window, cx| {
                                        window.dispatch_action(Box::new(PasteKubeconfig), cx)
                                    },
                                ))
                                .separator()
                                .item(
                                    PopupMenuItem::new("Manage kubeconfigs").on_click(
                                        |_, window, cx| {
                                            window.dispatch_action(Box::new(OpenClusters), cx)
                                        },
                                    ),
                                )
                            }),
                    )
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
