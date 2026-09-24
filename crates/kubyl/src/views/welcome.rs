use gpui::{
    App, Context, FocusHandle, Focusable, FontWeight, IntoElement, Render, SharedString, Window,
    div, img, prelude::*,
};
use kubyl_core::actions::{AddKubeconfig, OpenSettings};
use kubyl_core::{TabView, ViewKind, ViewRequest};
use kubyl_ui::{ActiveColors, Button, IconName, Kbd, h_flex, u, v_flex};

use super::dispatch_or_explain;

/// The empty state: "Add a kubeconfig". Replaced by a cluster list in phase 01.
pub struct WelcomeView {
    focus: FocusHandle,
}

impl WelcomeView {
    pub fn new(cx: &mut Context<Self>) -> Self {
        Self {
            focus: cx.focus_handle(),
        }
    }
}

impl Focusable for WelcomeView {
    fn focus_handle(&self, _: &App) -> FocusHandle {
        self.focus.clone()
    }
}

impl TabView for WelcomeView {
    fn tab_title(&self, _: &App) -> SharedString {
        "Welcome".into()
    }

    fn tab_icon(&self, _: &App) -> Option<SharedString> {
        Some(IconName::ShipWheel.path())
    }

    fn view_request(&self, _: &App) -> Option<ViewRequest> {
        Some(ViewRequest::new(ViewKind::Welcome))
    }
}

fn shortcut(keys: &str, label: &'static str, cx: &App) -> impl IntoElement {
    h_flex()
        .justify_between()
        .w(u(300.0))
        .text_color(cx.colors().text_muted)
        .child(label)
        .child(Kbd::keystroke(keys))
}

impl Render for WelcomeView {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let colors = cx.colors().clone();
        let secondary = if cfg!(target_os = "macos") {
            "cmd"
        } else {
            "ctrl"
        };
        v_flex()
            .id("welcome")
            .track_focus(&self.focus)
            .size_full()
            .overflow_y_scroll()
            .items_center()
            .justify_center()
            .gap(u(22.0))
            .p(u(24.0))
            .child(img("logo/png/app-icon-128.png").size(u(72.0)).flex_none())
            .child(
                v_flex()
                    .items_center()
                    .gap(u(6.0))
                    .child(
                        div()
                            .text_size(u(20.0))
                            .font_weight(FontWeight::SEMIBOLD)
                            .text_color(colors.text)
                            .child("Welcome to Kubyl"),
                    )
                    .child(
                        div()
                            .text_color(colors.text_dim)
                            .child("Add a kubeconfig to connect to your clusters."),
                    ),
            )
            .child(
                h_flex()
                    .gap(u(8.0))
                    .child(
                        Button::new("add-kubeconfig")
                            .primary()
                            .icon(IconName::FilePlus)
                            .label("Add kubeconfig…")
                            .on_click(|_, window, cx| {
                                dispatch_or_explain(
                                    Box::new(AddKubeconfig),
                                    "Adding kubeconfigs",
                                    window,
                                    cx,
                                )
                            }),
                    )
                    .child(
                        Button::new("open-settings")
                            .icon(IconName::Settings)
                            .label("Open settings")
                            .on_click(|_, window, cx| {
                                window.dispatch_action(Box::new(OpenSettings), cx)
                            }),
                    ),
            )
            .child(
                v_flex()
                    .gap(u(8.0))
                    .pt(u(10.0))
                    .text_size(u(12.0))
                    .child(shortcut(&format!("{secondary}-k"), "Search everything", cx))
                    .child(shortcut(&format!("{secondary}-b"), "Toggle sidebar", cx))
                    .child(shortcut(
                        &format!("{secondary}-j"),
                        "Toggle bottom dock",
                        cx,
                    ))
                    .child(shortcut(
                        &format!("{secondary}-r"),
                        "Toggle details dock",
                        cx,
                    ))
                    .child(shortcut("shift-escape", "Zoom pane", cx)),
            )
    }
}
