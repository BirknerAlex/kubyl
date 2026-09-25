//! The name-conflict dialog of copies: overwrite, keep both or skip, optionally for every
//! remaining conflict.

use std::cell::Cell;
use std::rc::Rc;

use gpui::{
    App, AppContext as _, Context, FocusHandle, Focusable, FontWeight, IntoElement, Window, div,
    prelude::*, px,
};
use gpui_component::Sizable as _;
use gpui_component::WindowExt as _;
use gpui_component::checkbox::Checkbox;
use gpui_component::dialog::Confirm;
use kubyl_ui::{ActiveColors, Button, fonts, h_flex, u, v_flex};

/// What to do with a name that exists at the destination.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Resolution {
    Overwrite,
    KeepBoth,
    Skip,
}

type OnChoice = Rc<dyn Fn(Resolution, bool, &mut Window, &mut App)>;

/// Asks what to do with `name` existing in `destination`. `remaining`: conflicts after this
/// one (shows "apply to all"). `on_choice(resolution, apply_to_all)`; closing the dialog skips.
pub fn conflict(
    name: String,
    destination: String,
    remaining: usize,
    on_choice: impl Fn(Resolution, bool, &mut Window, &mut App) + 'static,
    window: &mut Window,
    cx: &mut App,
) {
    let on_choice: OnChoice = Rc::new(on_choice);
    let decided = Rc::new(Cell::new(false));
    let view = cx.new(|cx| ConflictDialog {
        name,
        destination,
        remaining,
        apply_to_all: false,
        on_choice: on_choice.clone(),
        decided: decided.clone(),
        focus: cx.focus_handle(),
    });
    let colors = cx.colors().clone();
    let content = view.clone();
    window.open_dialog(cx, move |dialog, _, _| {
        dialog
            .w(px(460.0))
            .margin_top(px(90.0))
            .p_0()
            .bg(colors.panel)
            .close_button(false)
            // Enter is handled by the view; the dialog must not close itself on Confirm.
            .on_ok(|_, _, _| false)
            // Escape or a click outside skips this item, so the rest of the copy goes on.
            .on_close({
                let (on_choice, decided) = (on_choice.clone(), decided.clone());
                move |_, window, cx| {
                    if !decided.replace(true) {
                        on_choice(Resolution::Skip, false, window, cx);
                    }
                }
            })
            .content({
                let view = content.clone();
                move |content, _, _| content.child(view.clone())
            })
    });
    let focus = view.read(cx).focus.clone();
    window.focus(&focus, cx);
}

struct ConflictDialog {
    name: String,
    destination: String,
    remaining: usize,
    apply_to_all: bool,
    on_choice: OnChoice,
    /// Set once a choice (or a dismissal) continued the copy: it continues only once.
    decided: Rc<Cell<bool>>,
    focus: FocusHandle,
}

impl ConflictDialog {
    fn choose(&mut self, resolution: Resolution, window: &mut Window, cx: &mut Context<Self>) {
        if self.decided.replace(true) {
            return;
        }
        window.close_dialog(cx);
        (self.on_choice)(resolution, self.apply_to_all, window, cx);
    }
}

impl Focusable for ConflictDialog {
    fn focus_handle(&self, _: &App) -> FocusHandle {
        self.focus.clone()
    }
}

impl Render for ConflictDialog {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let colors = cx.colors().clone();
        v_flex()
            .track_focus(&self.focus)
            // The dialog binds enter to Confirm: keep both, the choice that never loses data.
            .on_action(cx.listener(|this, _: &Confirm, window, cx| {
                this.choose(Resolution::KeepBoth, window, cx)
            }))
            .text_color(colors.text)
            .text_size(u(13.0))
            .child(
                div()
                    .px(u(16.0))
                    .py(u(14.0))
                    .border_b_1()
                    .border_color(colors.border_variant)
                    .font_weight(FontWeight::SEMIBOLD)
                    .child("Replace the existing item?"),
            )
            .child(
                v_flex()
                    .p(u(16.0))
                    .gap(u(10.0))
                    .text_color(colors.text_muted)
                    .child(
                        h_flex()
                            .flex_wrap()
                            .gap(u(4.0))
                            .child(
                                div()
                                    .font_family(fonts::MONO)
                                    .text_color(colors.text)
                                    .child(self.name.clone()),
                            )
                            .child("already exists in")
                            .child(
                                div()
                                    .min_w_0()
                                    .truncate()
                                    .font_family(fonts::MONO)
                                    .text_color(colors.text)
                                    .child(self.destination.clone()),
                            ),
                    )
                    .child(
                        div()
                            .text_size(u(12.0))
                            .child("Keep both adds \" (1)\" to the new copy's name. Hold ⌥ while dropping to keep both without asking."),
                    )
                    .when(self.remaining > 0, |this| {
                        this.child(
                            Checkbox::new("conflict-apply-all")
                                .small()
                                .label(format!(
                                    "Apply to the {} other conflict{}",
                                    self.remaining,
                                    if self.remaining == 1 { "" } else { "s" }
                                ))
                                .checked(self.apply_to_all)
                                .on_click(cx.listener(|this, checked: &bool, _, cx| {
                                    this.apply_to_all = *checked;
                                    cx.notify();
                                })),
                        )
                    }),
            )
            .child(
                h_flex()
                    .justify_end()
                    .gap(u(8.0))
                    .px(u(16.0))
                    .py(u(12.0))
                    .border_t_1()
                    .border_color(colors.border_variant)
                    .child(Button::new("conflict-skip").ghost().label("Skip").on_click(
                        cx.listener(|this, _, window, cx| this.choose(Resolution::Skip, window, cx)),
                    ))
                    .child(Button::new("conflict-keep").label("Keep both").on_click(cx.listener(
                        |this, _, window, cx| this.choose(Resolution::KeepBoth, window, cx),
                    )))
                    .child(
                        Button::new("conflict-overwrite")
                            .danger()
                            .label("Overwrite")
                            .on_click(cx.listener(|this, _, window, cx| {
                                this.choose(Resolution::Overwrite, window, cx)
                            })),
                    ),
            )
    }
}
