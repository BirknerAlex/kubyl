//! The `helm` commands of a release, to copy (phase 12's dialog; the secondary action next to
//! Kubyl's own upgrade, rollback and uninstall).

use gpui::{
    App, AppContext as _, Context, FocusHandle, Focusable, IntoElement, Render, Window, div,
    prelude::*,
};
use gpui_component::WindowExt as _;
use kubyl_ui::{ActiveColors, Icon, IconName, fonts, h_flex, u, v_flex};

use super::{close_button, footer, header, open};
use crate::present::Command;
use crate::widgets;

/// Opens the list of `helm` commands for a release, to copy.
pub fn open_helm_commands(
    title: String,
    commands: Vec<Command>,
    window: &mut Window,
    cx: &mut App,
) {
    let view = cx.new(|cx| HelmCommands {
        title,
        commands,
        focus: cx.focus_handle(),
    });
    let focus = view.read(cx).focus.clone();
    open(view, 620.0, Some(focus), window, cx);
}

struct HelmCommands {
    title: String,
    commands: Vec<Command>,
    focus: FocusHandle,
}

impl Focusable for HelmCommands {
    fn focus_handle(&self, _: &App) -> FocusHandle {
        self.focus.clone()
    }
}

impl Render for HelmCommands {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let colors = cx.colors().clone();
        v_flex()
            .track_focus(&self.focus)
            .text_color(colors.text)
            .text_size(u(13.0))
            .child(header(
                Icon::new(IconName::Anchor)
                    .size(16.0)
                    .color(colors.accent)
                    .into_any_element(),
                self.title.clone(),
                Vec::new(),
                &colors,
            ))
            .child(
                v_flex()
                    .p(u(8.0))
                    .gap(u(2.0))
                    .children(self.commands.iter().enumerate().map(|(i, command)| {
                        let text = command.command.clone();
                        let label = command.label.clone();
                        h_flex()
                            .id(("helm-command", i))
                            .items_start()
                            .gap(u(10.0))
                            .px(u(10.0))
                            .py(u(6.0))
                            .rounded(u(5.0))
                            .cursor_pointer()
                            .hover(|s| s.bg(colors.hover))
                            .child(
                                v_flex()
                                    .flex_1()
                                    .min_w_0()
                                    .child(div().text_size(u(12.5)).child(command.label.clone()))
                                    .child(
                                        div()
                                            .font_family(fonts::MONO)
                                            .text_size(u(11.0))
                                            .text_color(colors.text_dim)
                                            .child(command.command.clone()),
                                    ),
                            )
                            .child(Icon::new(IconName::Copy).size(13.0).color(colors.text_dim))
                            .on_click(move |_, window, cx| {
                                widgets::copy(
                                    text.clone(),
                                    &format!("the {} command", label.to_lowercase()),
                                    cx,
                                );
                                window.close_dialog(cx);
                            })
                    })),
            )
            .child(footer(
                Some(
                    div()
                        .text_size(u(11.5))
                        .text_color(colors.text_dim)
                        .child("The command goes to the clipboard: run it in a terminal.")
                        .into_any_element(),
                ),
                vec![close_button("Close")],
                &colors,
            ))
    }
}
