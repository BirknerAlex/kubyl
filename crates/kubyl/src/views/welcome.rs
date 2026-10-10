use gpui::{
    App, Context, FocusHandle, Focusable, FontWeight, IntoElement, Render, SharedString, Window,
    div, img, prelude::*,
};
use kubyl_core::actions::{AddKubeconfig, OpenSettings};
use kubyl_core::{TabView, ViewKind, ViewRequest};
use kubyl_settings::{State, StateSection};
use kubyl_ui::{ActiveColors, Button, IconButton, IconName, Kbd, h_flex, u, v_flex};
use serde::{Deserialize, Serialize};

use crate::app::OpenMobile;

/// What state.json stores under `promo`: notices the user has dismissed for good.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(default)]
struct PromoState {
    mobile_dismissed: bool,
}

impl StateSection for PromoState {
    const KEY: &'static str = "promo";
}

/// The empty state: "Add a kubeconfig". Replaced by a cluster list in phase 01.
pub struct WelcomeView {
    focus: FocusHandle,
    show_mobile: bool,
}

impl WelcomeView {
    pub fn new(cx: &mut Context<Self>) -> Self {
        Self {
            focus: cx.focus_handle(),
            show_mobile: !State::get::<PromoState>(cx).mobile_dismissed,
        }
    }

    fn dismiss_mobile(&mut self, cx: &mut Context<Self>) {
        self.show_mobile = false;
        State::set(
            cx,
            &PromoState {
                mobile_dismissed: true,
            },
        );
        cx.notify();
    }

    /// A quiet, dismissible hint: one line and a link, shown only here and never as a toast.
    fn mobile_notice(&self, cx: &mut Context<Self>) -> impl IntoElement {
        let colors = cx.colors().clone();
        h_flex()
            .id("mobile-notice")
            .w(u(440.0))
            .max_w_full()
            .items_center()
            .gap(u(10.0))
            .px(u(12.0))
            .py(u(8.0))
            .rounded(u(6.0))
            .border_1()
            .border_color(colors.border_variant)
            .bg(colors.panel)
            .text_size(u(12.0))
            .child(
                v_flex()
                    .flex_1()
                    .min_w_0()
                    .gap(u(2.0))
                    .child(
                        div()
                            .font_weight(FontWeight::MEDIUM)
                            .text_color(colors.text)
                            .child("Kubyl Mobile for iOS and Android"),
                    )
                    .child(div().text_color(colors.text_dim).child(
                        "Check pods, logs and alerts from your phone. Needs a Kubyl Server \
                         next to your clusters; this app doesn't.",
                    )),
            )
            .child(
                Button::new("mobile-learn-more")
                    .icon(IconName::ExternalLink)
                    .label("Learn more")
                    .on_click(|_, window, cx| window.dispatch_action(Box::new(OpenMobile), cx)),
            )
            .child(
                IconButton::new("mobile-dismiss", IconName::X)
                    .icon_size(12.0)
                    .on_click(cx.listener(|this, _, _, cx| this.dismiss_mobile(cx))),
            )
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
        let show_mobile = self.show_mobile;
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
                                window.dispatch_action(Box::new(AddKubeconfig), cx)
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
            .when(show_mobile, |this| this.child(self.mobile_notice(cx)))
    }
}
