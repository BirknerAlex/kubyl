//! How a proposed change looks: its summary, warnings and diffs, and the dialog that asks the
//! user (for changes from outside clients, and for production clusters).

use std::rc::Rc;

use gpui::{
    AnyElement, App, AppContext as _, Context, Entity, FocusHandle, Focusable, FontWeight,
    IntoElement, Render, SharedString, Subscription, Window, div, prelude::*, px,
};
use gpui_component::WindowExt as _;
use gpui_component::input::{Input, InputEvent, InputState};
use kubyl_mcp_core::approval::{ChangeDoc, Decision, Proposal};
use kubyl_ui::{
    ActiveColors as _, Button, Colors, Icon, IconButton, IconName, ProdBadge, fonts, h_flex, u,
    v_flex,
};

/// Most diff lines shown per object.
const MAX_LINES: usize = 400;

fn small(text: impl Into<SharedString>, color: gpui::Hsla) -> impl IntoElement {
    div()
        .text_size(u(11.5))
        .text_color(color)
        .child(text.into())
}

/// A line diff of `old` against `new` under `label`.
pub fn diff_block(label: &str, old: Option<&str>, new: &str, colors: &Colors) -> AnyElement {
    let diff = similar::TextDiff::from_lines(old.unwrap_or(""), new);
    let mut lines: Vec<AnyElement> = Vec::new();
    for change in diff.iter_all_changes().take(MAX_LINES) {
        let (sign, color, bg) = match change.tag() {
            similar::ChangeTag::Delete => ("-", colors.red, Some(colors.red.opacity(0.08))),
            similar::ChangeTag::Insert => ("+", colors.green, Some(colors.green.opacity(0.08))),
            similar::ChangeTag::Equal => (" ", colors.text_dim, None),
        };
        let text = format!("{sign} {}", change.value().trim_end_matches('\n'));
        lines.push(
            div()
                .px(u(6.0))
                .when_some(bg, |this, bg| this.bg(bg))
                .text_color(color)
                .child(text)
                .into_any_element(),
        );
    }
    v_flex()
        .gap(u(2.0))
        .child(small(label.to_string(), colors.text_dim))
        .child(
            v_flex()
                .rounded(u(4.0))
                .border_1()
                .border_color(colors.border_variant)
                .bg(colors.background)
                .py(u(4.0))
                .font_family(fonts::MONO)
                .text_size(u(11.5))
                .children(lines),
        )
        .into_any_element()
}

/// A changed object: the diff, or for a deleted one what goes away.
fn doc_block(doc: &ChangeDoc, colors: &Colors) -> AnyElement {
    diff_block(&doc.label, doc.old.as_deref(), &doc.new, colors)
}

/// The cluster, a PROD badge, the warnings and the diffs of a proposal.
pub fn proposal_body(proposal: &Proposal, colors: &Colors) -> AnyElement {
    v_flex()
        .gap(u(8.0))
        .child(
            h_flex()
                .gap(u(6.0))
                .text_size(u(12.0))
                .text_color(colors.text_muted)
                .child("on")
                .child(
                    div()
                        .font_weight(FontWeight::SEMIBOLD)
                        .text_color(colors.text)
                        .child(proposal.cluster_name.clone()),
                )
                .when(proposal.production, |this| this.child(ProdBadge)),
        )
        .children(proposal.warnings.iter().map(|warning| {
            h_flex()
                .gap(u(6.0))
                .items_start()
                .text_size(u(12.0))
                .text_color(colors.yellow)
                .child(
                    Icon::new(IconName::TriangleAlert)
                        .size(13.0)
                        .color(colors.yellow),
                )
                .child(div().flex_1().min_w_0().child(warning.clone()))
        }))
        .children(proposal.docs.iter().map(|doc| doc_block(doc, colors)))
        .into_any_element()
}

type OnDecision = Rc<dyn Fn(Decision, &mut Window, &mut App)>;

/// Opens the dialog that asks whether `proposal` may happen. `on_decision` runs once, with
/// the user's choice. A production cluster needs its name typed before "Apply" works.
/// The dialog can only be closed by choosing.
pub fn ask(
    proposal: Proposal,
    source: impl Into<SharedString>,
    on_decision: impl Fn(Decision, &mut Window, &mut App) + 'static,
    window: &mut Window,
    cx: &mut App,
) {
    let view =
        cx.new(|cx| AskDialog::new(proposal, source.into(), Rc::new(on_decision), window, cx));
    let colors = cx.colors().clone();
    window.open_dialog(cx, {
        let view = view.clone();
        move |dialog, _, _| {
            dialog
                .w(px(640.0))
                .margin_top(px(70.0))
                .p_0()
                .bg(colors.panel)
                .close_button(false)
                .overlay_closable(false)
                .keyboard(false)
                .on_ok(|_, _, _| false)
                .content({
                    let view = view.clone();
                    move |content, _, _| content.child(view.clone())
                })
        }
    });
    let focus = view.read(cx).focus.clone();
    window.focus(&focus, cx);
}

struct AskDialog {
    proposal: Proposal,
    source: SharedString,
    typed: Entity<InputState>,
    on_decision: OnDecision,
    focus: FocusHandle,
    _subscriptions: Vec<Subscription>,
}

impl AskDialog {
    fn new(
        proposal: Proposal,
        source: SharedString,
        on_decision: OnDecision,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Self {
        let typed =
            cx.new(|cx| InputState::new(window, cx).placeholder(proposal.cluster_name.clone()));
        let subscription = cx.subscribe_in(
            &typed,
            window,
            |this, _, event: &InputEvent, window, cx| match event {
                InputEvent::Change => cx.notify(),
                InputEvent::PressEnter { .. } => this.decide(Decision::Approved, window, cx),
                _ => {}
            },
        );
        Self {
            proposal,
            source,
            typed,
            on_decision,
            focus: cx.focus_handle(),
            _subscriptions: vec![subscription],
        }
    }

    fn approvable(&self, cx: &App) -> bool {
        !self.proposal.production
            || self.typed.read(cx).value().trim() == self.proposal.cluster_name
    }

    fn decide(&mut self, decision: Decision, window: &mut Window, cx: &mut Context<Self>) {
        if decision == Decision::Approved && !self.approvable(cx) {
            return;
        }
        window.close_dialog(cx);
        (self.on_decision)(decision, window, cx);
    }
}

impl Focusable for AskDialog {
    fn focus_handle(&self, _: &App) -> FocusHandle {
        self.focus.clone()
    }
}

impl Render for AskDialog {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let colors = cx.colors().clone();
        let production = self.proposal.production;
        let approvable = self.approvable(cx);
        let name = self.proposal.cluster_name.clone();
        v_flex()
            .child(
                v_flex()
                    .gap(u(2.0))
                    .px(u(16.0))
                    .py(u(14.0))
                    .border_b_1()
                    .border_color(colors.border_variant)
                    .child(
                        h_flex()
                            .gap(u(6.0))
                            .font_weight(FontWeight::SEMIBOLD)
                            .child(Icon::new(IconName::Diff).size(14.0).color(colors.yellow))
                            .child(self.proposal.title.clone()),
                    )
                    .child(small(
                        format!(
                            "{} wants to change a cluster. Nothing happens until you apply it.",
                            self.source
                        ),
                        colors.text_dim,
                    )),
            )
            .child(
                div()
                    .id("mcp-change-body")
                    .p(u(16.0))
                    .max_h(px(420.0))
                    .overflow_y_scroll()
                    .child(proposal_body(&self.proposal, &colors)),
            )
            .when(production, |this| {
                this.child(
                    v_flex()
                        .gap(u(6.0))
                        .px(u(16.0))
                        .pb(u(12.0))
                        .child(
                            h_flex()
                                .flex_wrap()
                                .gap(u(6.0))
                                .text_size(u(12.0))
                                .text_color(colors.text_muted)
                                .child(ProdBadge)
                                .child("This is a production cluster. Type")
                                .child(
                                    div()
                                        .font_family(fonts::MONO)
                                        .text_color(colors.text)
                                        .child(name.clone()),
                                )
                                .child(
                                    IconButton::new("mcp-typed-copy", IconName::Copy)
                                        .icon_size(11.0)
                                        .on_click(move |_, _, cx| {
                                            cx.write_to_clipboard(gpui::ClipboardItem::new_string(
                                                name.clone(),
                                            ))
                                        }),
                                )
                                .child("to apply."),
                        )
                        .child(
                            div()
                                .h(u(28.0))
                                .px(u(8.0))
                                .flex()
                                .items_center()
                                .rounded(u(5.0))
                                .bg(colors.input_background)
                                .border_1()
                                .border_color(colors.border)
                                .font_family(fonts::MONO)
                                .text_size(u(12.5))
                                .child(Input::new(&self.typed).appearance(false)),
                        ),
                )
            })
            .child(
                h_flex()
                    .justify_end()
                    .gap(u(8.0))
                    .px(u(16.0))
                    .py(u(12.0))
                    .border_t_1()
                    .border_color(colors.border_variant)
                    .child(
                        Button::new("mcp-reject")
                            .label("Reject")
                            .ghost()
                            .on_click(cx.listener(|this, _, window, cx| {
                                this.decide(Decision::Rejected, window, cx)
                            })),
                    )
                    .child({
                        let button = Button::new("mcp-apply")
                            .label(if production {
                                "Apply to production"
                            } else {
                                "Apply"
                            })
                            .disabled(!approvable)
                            .on_click(cx.listener(|this, _, window, cx| {
                                this.decide(Decision::Approved, window, cx)
                            }));
                        if production {
                            button.danger()
                        } else {
                            button.primary()
                        }
                    }),
            )
    }
}
