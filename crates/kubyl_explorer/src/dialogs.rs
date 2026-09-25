//! Dialogs for resource actions: confirmation (typed on PROD clusters, optional grace period),
//! scale, rollout undo and drain progress.

use std::rc::Rc;

use gpui::{
    App, AppContext as _, Context, Entity, FocusHandle, Focusable, FontWeight, IntoElement, Render,
    SharedString, Subscription, Window, div, prelude::*, px,
};
use gpui_component::WindowExt as _;
use gpui_component::input::{Input, InputEvent, InputState};
use kubyl_resources::ops::{DrainProgress, Revision};
use kubyl_ui::{ActiveColors, Button, ProgressBar, StatusDot, fonts, h_flex, u, v_flex};

/// What the user entered in a confirmation dialog.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct ConfirmResult {
    /// Parsed grace period (when the dialog asked for one; `None` = default).
    pub grace_period: Option<u32>,
    /// Parsed number (scale dialog).
    pub number: Option<u32>,
}

type OnConfirm = Rc<dyn Fn(ConfirmResult, &mut Window, &mut App)>;

/// A confirmation dialog.
pub struct ConfirmSpec {
    pub title: SharedString,
    /// Lines shown in the body (the objects affected).
    pub lines: Vec<SharedString>,
    pub note: Option<SharedString>,
    pub confirm_label: SharedString,
    pub danger: bool,
    /// The user must type this text to enable the button (PROD clusters).
    pub typed: Option<String>,
    /// Show a grace-period input with this default (`Some(None)`: empty = object default).
    pub grace_period: Option<Option<u32>>,
    /// Show a number input with this value (scale).
    pub number: Option<u32>,
}

impl ConfirmSpec {
    pub fn new(title: impl Into<SharedString>, confirm_label: impl Into<SharedString>) -> Self {
        Self {
            title: title.into(),
            lines: Vec::new(),
            note: None,
            confirm_label: confirm_label.into(),
            danger: false,
            typed: None,
            grace_period: None,
            number: None,
        }
    }
}

/// Opens a dialog; `on_confirm` runs when the user confirms.
pub fn confirm(
    spec: ConfirmSpec,
    on_confirm: impl Fn(ConfirmResult, &mut Window, &mut App) + 'static,
    window: &mut Window,
    cx: &mut App,
) {
    let view = cx.new(|cx| ConfirmDialog::new(spec, Rc::new(on_confirm), window, cx));
    open(view.clone(), 480.0, window, cx);
    let focus = view.read(cx).initial_focus(cx);
    window.focus(&focus, cx);
}

fn open<V: Render>(view: Entity<V>, width: f32, window: &mut Window, cx: &mut App) {
    let colors = cx.colors().clone();
    window.open_dialog(cx, move |dialog, _, _| {
        dialog
            .w(px(width))
            .margin_top(px(90.0))
            .p_0()
            .bg(colors.panel)
            .close_button(false)
            // Enter in an input propagates to the dialog's Confirm binding, which would close
            // the dialog (dropping the view) before the input's `PressEnter` reaches it. The
            // views submit on `PressEnter` themselves.
            .on_ok(|_, _, _| false)
            // As content, not a child: children go into a scroll body whose height collapses,
            // which clipped the hitboxes of the footer (presses on the confirm buttons reached
            // the backdrop and closed the dialog).
            .content({
                let view = view.clone();
                move |content, _, _| content.child(view.clone())
            })
    });
}

struct ConfirmDialog {
    spec: ConfirmSpec,
    typed: Entity<InputState>,
    grace: Entity<InputState>,
    number: Entity<InputState>,
    on_confirm: OnConfirm,
    error: Option<String>,
    focus: FocusHandle,
    _subscriptions: Vec<Subscription>,
}

impl ConfirmDialog {
    fn new(
        spec: ConfirmSpec,
        on_confirm: OnConfirm,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Self {
        let typed = cx.new(|cx| {
            InputState::new(window, cx).placeholder(spec.typed.clone().unwrap_or_default())
        });
        let grace = cx.new(|cx| {
            let mut state = InputState::new(window, cx).placeholder("default");
            if let Some(Some(seconds)) = spec.grace_period {
                state.set_value(seconds.to_string(), window, cx);
            }
            state
        });
        let number = cx.new(|cx| {
            let mut state = InputState::new(window, cx);
            if let Some(n) = spec.number {
                state.set_value(n.to_string(), window, cx);
            }
            state
        });
        let mut subscriptions = Vec::new();
        for input in [&typed, &grace, &number] {
            subscriptions.push(cx.subscribe_in(
                input,
                window,
                |this, _, event: &InputEvent, window, cx| match event {
                    InputEvent::Change => {
                        this.error = None;
                        cx.notify();
                    }
                    InputEvent::PressEnter { .. } => this.submit(window, cx),
                    _ => {}
                },
            ));
        }
        Self {
            spec,
            typed,
            grace,
            number,
            on_confirm,
            error: None,
            focus: cx.focus_handle(),
            _subscriptions: subscriptions,
        }
    }

    fn initial_focus(&self, cx: &App) -> FocusHandle {
        if self.spec.typed.is_some() {
            self.typed.read(cx).focus_handle(cx)
        } else if self.spec.number.is_some() {
            self.number.read(cx).focus_handle(cx)
        } else {
            self.focus.clone()
        }
    }

    fn typed_ok(&self, cx: &App) -> bool {
        match &self.spec.typed {
            Some(expected) => self.typed.read(cx).value().trim() == expected,
            None => true,
        }
    }

    fn submit(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if !self.typed_ok(cx) {
            self.error = Some("Type the text shown to confirm.".into());
            cx.notify();
            return;
        }
        let mut result = ConfirmResult::default();
        if self.spec.grace_period.is_some() {
            let text = self.grace.read(cx).value().trim().to_string();
            if !text.is_empty() {
                match text.parse::<u32>() {
                    Ok(seconds) => result.grace_period = Some(seconds),
                    Err(_) => {
                        self.error = Some("The grace period must be a number of seconds.".into());
                        cx.notify();
                        return;
                    }
                }
            }
        }
        if self.spec.number.is_some() {
            match self.number.read(cx).value().trim().parse::<u32>() {
                Ok(n) => result.number = Some(n),
                Err(_) => {
                    self.error = Some("Enter a whole number.".into());
                    cx.notify();
                    return;
                }
            }
        }
        window.close_dialog(cx);
        (self.on_confirm)(result, window, cx);
    }
}

impl Focusable for ConfirmDialog {
    fn focus_handle(&self, _: &App) -> FocusHandle {
        self.focus.clone()
    }
}

fn header(title: SharedString, cx: &App) -> impl IntoElement {
    let colors = cx.colors();
    div()
        .px(u(16.0))
        .py(u(14.0))
        .border_b_1()
        .border_color(colors.border_variant)
        .font_weight(FontWeight::SEMIBOLD)
        .child(title)
}

/// The PROD notice and the input where the object's name must be typed.
fn typed_confirmation(
    typed: SharedString,
    input: &Entity<InputState>,
    cx: &App,
) -> impl IntoElement {
    let colors = cx.colors();
    v_flex()
        .gap(u(6.0))
        .child(
            h_flex()
                .gap(u(6.0))
                .text_size(u(12.0))
                .child(kubyl_ui::ProdBadge)
                .child("This is a production cluster. Type")
                .child(
                    div()
                        .font_family(fonts::MONO)
                        .text_color(colors.text)
                        .child(typed),
                )
                .child("to confirm."),
        )
        .child(labelled(
            "Confirmation",
            Input::new(input).appearance(false),
            cx,
        ))
}

fn labelled(label: &'static str, input: impl IntoElement, cx: &App) -> impl IntoElement {
    let colors = cx.colors();
    v_flex()
        .gap(u(4.0))
        .child(
            div()
                .text_size(u(12.0))
                .text_color(colors.text_dim)
                .child(label),
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
                .child(input),
        )
}

impl Render for ConfirmDialog {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let colors = cx.colors().clone();
        let enabled = self.typed_ok(cx);
        let mut body = v_flex()
            .p(u(16.0))
            .gap(u(10.0))
            .text_color(colors.text_muted);
        if !self.spec.lines.is_empty() {
            let shown = self.spec.lines.len().min(8);
            let mut list = v_flex()
                .gap(u(2.0))
                .font_family(fonts::MONO)
                .text_size(u(12.0))
                .text_color(colors.text);
            for line in &self.spec.lines[..shown] {
                list = list.child(div().truncate().child(line.clone()));
            }
            if self.spec.lines.len() > shown {
                list = list.child(
                    div()
                        .text_color(colors.text_dim)
                        .child(format!("… and {} more", self.spec.lines.len() - shown)),
                );
            }
            body = body.child(list);
        }
        if let Some(note) = &self.spec.note {
            body = body.child(div().text_size(u(12.0)).child(note.clone()));
        }
        if self.spec.number.is_some() {
            body = body.child(labelled(
                "Replicas",
                Input::new(&self.number).appearance(false),
                cx,
            ));
        }
        if self.spec.grace_period.is_some() {
            body = body.child(labelled(
                "Grace period (seconds, empty = default)",
                Input::new(&self.grace).appearance(false),
                cx,
            ));
        }
        if let Some(typed) = &self.spec.typed {
            body = body.child(typed_confirmation(typed.clone().into(), &self.typed, cx));
        }
        if let Some(error) = &self.error {
            body = body.child(
                div()
                    .text_size(u(12.0))
                    .text_color(colors.red)
                    .child(error.clone()),
            );
        }
        let confirm = Button::new("confirm")
            .label(self.spec.confirm_label.clone())
            .disabled(!enabled)
            .on_click(cx.listener(|this, _, window, cx| this.submit(window, cx)));
        let confirm = if self.spec.danger {
            confirm.danger()
        } else {
            confirm.primary()
        };
        v_flex()
            .track_focus(&self.focus)
            .text_color(colors.text)
            .text_size(u(13.0))
            .child(header(self.spec.title.clone(), cx))
            .child(body)
            .child(
                h_flex()
                    .justify_end()
                    .gap(u(8.0))
                    .px(u(16.0))
                    .py(u(12.0))
                    .border_t_1()
                    .border_color(colors.border_variant)
                    .child(
                        Button::new("cancel")
                            .ghost()
                            .label("Cancel")
                            .on_click(|_, window, cx| window.close_dialog(cx)),
                    )
                    .child(confirm),
            )
    }
}

// ----- Rollout undo -----

type OnRevision = Rc<dyn Fn(i64, &mut Window, &mut App)>;

/// Lists a workload's revisions (loaded by `load`) and calls `on_pick` with the chosen one.
pub fn pick_revision(
    title: SharedString,
    typed: Option<String>,
    load: gpui::Task<Result<Vec<Revision>, String>>,
    on_pick: impl Fn(i64, &mut Window, &mut App) + 'static,
    window: &mut Window,
    cx: &mut App,
) {
    let view = cx.new(|cx| RevisionDialog::new(title, typed, Rc::new(on_pick), window, cx));
    let weak = view.downgrade();
    cx.spawn(async move |cx| {
        let revisions = load.await;
        weak.update(cx, |this, cx| {
            this.revisions = Some(revisions);
            // Default to the revision before the current one, like `kubectl rollout undo`.
            if let Some(Ok(list)) = &this.revisions {
                this.selected = list.iter().position(|r| !r.current);
            }
            cx.notify();
        })
        .ok();
    })
    .detach();
    open(view.clone(), 560.0, window, cx);
    let focus = view.read(cx).focus.clone();
    window.focus(&focus, cx);
}

struct RevisionDialog {
    title: SharedString,
    typed: Option<String>,
    typed_input: Entity<InputState>,
    revisions: Option<Result<Vec<Revision>, String>>,
    selected: Option<usize>,
    on_pick: OnRevision,
    focus: FocusHandle,
}

impl RevisionDialog {
    fn new(
        title: SharedString,
        typed: Option<String>,
        on_pick: OnRevision,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Self {
        let typed_input =
            cx.new(|cx| InputState::new(window, cx).placeholder(typed.clone().unwrap_or_default()));
        Self {
            title,
            typed,
            typed_input,
            revisions: None,
            selected: None,
            on_pick,
            focus: cx.focus_handle(),
        }
    }

    fn submit(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Some(Ok(revisions)) = &self.revisions else {
            return;
        };
        let Some(revision) = self.selected.and_then(|i| revisions.get(i)) else {
            return;
        };
        if let Some(typed) = &self.typed
            && self.typed_input.read(cx).value().trim() != typed
        {
            return;
        }
        let revision = revision.revision;
        window.close_dialog(cx);
        (self.on_pick)(revision, window, cx);
    }
}

impl Focusable for RevisionDialog {
    fn focus_handle(&self, _: &App) -> FocusHandle {
        self.focus.clone()
    }
}

impl Render for RevisionDialog {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let colors = cx.colors().clone();
        let mut body = v_flex().p(u(12.0)).gap(u(2.0)).max_h(u(360.0));
        match &self.revisions {
            None => {
                body = body.child(
                    div()
                        .p(u(8.0))
                        .text_color(colors.text_dim)
                        .child("Loading revisions…"),
                )
            }
            Some(Err(err)) => {
                body = body.child(div().p(u(8.0)).text_color(colors.red).child(err.clone()))
            }
            Some(Ok(list)) if list.is_empty() => {
                body = body.child(
                    div()
                        .p(u(8.0))
                        .text_color(colors.text_dim)
                        .child("No revisions found."),
                )
            }
            Some(Ok(list)) => {
                for (ix, revision) in list.iter().enumerate() {
                    let selected = self.selected == Some(ix);
                    let hover = colors.hover;
                    body = body.child(
                        h_flex()
                            .id(("revision", ix))
                            .gap(u(10.0))
                            .px(u(10.0))
                            .py(u(6.0))
                            .rounded(u(5.0))
                            .cursor_pointer()
                            .when(selected, |this| this.bg(colors.selection))
                            .when(!selected, |this| this.hover(move |s| s.bg(hover)))
                            .on_click(cx.listener(move |this, _, _, cx| {
                                this.selected = Some(ix);
                                cx.notify();
                            }))
                            .child(
                                div()
                                    .w(u(44.0))
                                    .font_family(fonts::MONO)
                                    .text_color(if revision.current {
                                        colors.green
                                    } else {
                                        colors.text
                                    })
                                    .child(format!("#{}", revision.revision)),
                            )
                            .child(
                                v_flex()
                                    .flex_1()
                                    .min_w_0()
                                    .child(
                                        div()
                                            .truncate()
                                            .font_family(fonts::MONO)
                                            .text_size(u(12.0))
                                            .child(revision.images.join(", ")),
                                    )
                                    .child(
                                        div()
                                            .truncate()
                                            .text_size(u(11.5))
                                            .text_color(colors.text_dim)
                                            .child(format!(
                                                "{}{}",
                                                revision.replica_set,
                                                revision
                                                    .change_cause
                                                    .as_ref()
                                                    .map(|c| format!(" · {c}"))
                                                    .unwrap_or_default()
                                            )),
                                    ),
                            )
                            .when(revision.current, |this| {
                                this.child(
                                    div()
                                        .text_size(u(11.5))
                                        .text_color(colors.green)
                                        .child("current"),
                                )
                            }),
                    );
                }
            }
        }
        let can_submit = self.selected.is_some()
            && self
                .typed
                .as_ref()
                .is_none_or(|t| self.typed_input.read(cx).value().trim() == t);
        v_flex()
            .track_focus(&self.focus)
            .text_color(colors.text)
            .text_size(u(13.0))
            .child(header(self.title.clone(), cx))
            .child(div().id("revisions").overflow_y_scroll().child(body))
            .when_some(self.typed.clone(), |this, typed| {
                this.child(div().px(u(16.0)).pb(u(10.0)).child(typed_confirmation(
                    typed.into(),
                    &self.typed_input,
                    cx,
                )))
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
                        Button::new("cancel")
                            .ghost()
                            .label("Cancel")
                            .on_click(|_, window, cx| window.close_dialog(cx)),
                    )
                    .child(
                        Button::new("undo")
                            .primary()
                            .label("Roll back")
                            .disabled(!can_submit)
                            .on_click(cx.listener(|this, _, window, cx| this.submit(window, cx))),
                    ),
            )
    }
}

// ----- Drain progress -----

/// Live progress of a node drain.
pub struct DrainDialog {
    node: SharedString,
    progress: DrainProgress,
    focus: FocusHandle,
}

impl DrainDialog {
    pub fn open(node: SharedString, window: &mut Window, cx: &mut App) -> Entity<Self> {
        let view = cx.new(|cx| Self {
            node,
            progress: DrainProgress::default(),
            focus: cx.focus_handle(),
        });
        open(view.clone(), 480.0, window, cx);
        view
    }

    pub fn update(&mut self, progress: DrainProgress, cx: &mut Context<Self>) {
        self.progress = progress;
        cx.notify();
    }
}

impl Focusable for DrainDialog {
    fn focus_handle(&self, _: &App) -> FocusHandle {
        self.focus.clone()
    }
}

impl Render for DrainDialog {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let colors = cx.colors().clone();
        let p = &self.progress;
        let percent = if p.total == 0 {
            if p.done { 100.0 } else { 0.0 }
        } else {
            p.evicted as f32 / p.total as f32 * 100.0
        };
        let state = if let Some(error) = &p.error {
            (colors.red, error.clone())
        } else if p.done {
            (colors.green, "Drained.".to_string())
        } else if !p.blocked.is_empty() {
            (
                colors.yellow,
                format!(
                    "{} eviction(s) blocked by a PodDisruptionBudget, retrying…",
                    p.blocked.len()
                ),
            )
        } else if p.terminating > 0 {
            (
                colors.accent,
                format!("Waiting for {} pod(s) to terminate…", p.terminating),
            )
        } else {
            (colors.accent, "Evicting pods…".to_string())
        };
        v_flex()
            .track_focus(&self.focus)
            .text_color(colors.text)
            .text_size(u(13.0))
            .child(header(format!("Draining {}", self.node).into(), cx))
            .child(
                v_flex()
                    .p(u(16.0))
                    .gap(u(10.0))
                    .child(
                        h_flex()
                            .justify_between()
                            .text_size(u(12.0))
                            .text_color(colors.text_muted)
                            .child(format!("{} of {} pods evicted", p.evicted, p.total))
                            .child(format!("{} DaemonSet/mirror pods skipped", p.skipped)),
                    )
                    .child(ProgressBar::new(percent))
                    .child(
                        h_flex()
                            .gap(u(8.0))
                            .text_size(u(12.0))
                            .child(StatusDot::new(state.0))
                            .child(div().text_color(state.0).child(state.1)),
                    )
                    .children(p.blocked.iter().take(6).map(|pod| {
                        div()
                            .font_family(fonts::MONO)
                            .text_size(u(11.5))
                            .text_color(colors.text_dim)
                            .child(pod.clone())
                    })),
            )
            .child(
                h_flex()
                    .justify_end()
                    .px(u(16.0))
                    .py(u(12.0))
                    .border_t_1()
                    .border_color(colors.border_variant)
                    .child(
                        Button::new("close")
                            .label(if p.done { "Close" } else { "Hide" })
                            .on_click(|_, window, cx| window.close_dialog(cx)),
                    ),
            )
    }
}

// ----- Text prompt -----

type OnText = Rc<dyn Fn(String, &mut Window, &mut App)>;

/// Asks for one line of text (rename a favorite…).
pub fn prompt_text(
    title: SharedString,
    label: &'static str,
    initial: String,
    on_submit: impl Fn(String, &mut Window, &mut App) + 'static,
    window: &mut Window,
    cx: &mut App,
) {
    let view = cx.new(|cx| {
        let input = cx.new(|cx| {
            let mut state = InputState::new(window, cx);
            state.set_value(initial, window, cx);
            state
        });
        let subscription = cx.subscribe_in(
            &input,
            window,
            |this: &mut PromptDialog, _, event: &InputEvent, window, cx| {
                if let InputEvent::PressEnter { .. } = event {
                    this.submit(window, cx);
                }
            },
        );
        PromptDialog {
            title,
            label,
            input,
            on_submit: Rc::new(on_submit),
            focus: cx.focus_handle(),
            _subscription: subscription,
        }
    });
    open(view.clone(), 420.0, window, cx);
    let focus = view.read(cx).input.read(cx).focus_handle(cx);
    window.focus(&focus, cx);
}

struct PromptDialog {
    title: SharedString,
    label: &'static str,
    input: Entity<InputState>,
    on_submit: OnText,
    focus: FocusHandle,
    _subscription: Subscription,
}

impl PromptDialog {
    fn submit(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let text = self.input.read(cx).value().trim().to_string();
        window.close_dialog(cx);
        (self.on_submit)(text, window, cx);
    }
}

impl Focusable for PromptDialog {
    fn focus_handle(&self, _: &App) -> FocusHandle {
        self.focus.clone()
    }
}

impl Render for PromptDialog {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let colors = cx.colors().clone();
        v_flex()
            .track_focus(&self.focus)
            .text_color(colors.text)
            .text_size(u(13.0))
            .child(header(self.title.clone(), cx))
            .child(div().p(u(16.0)).child(labelled(
                self.label,
                Input::new(&self.input).appearance(false),
                cx,
            )))
            .child(
                h_flex()
                    .justify_end()
                    .gap(u(8.0))
                    .px(u(16.0))
                    .py(u(12.0))
                    .border_t_1()
                    .border_color(colors.border_variant)
                    .child(
                        Button::new("cancel")
                            .ghost()
                            .label("Cancel")
                            .on_click(|_, window, cx| window.close_dialog(cx)),
                    )
                    .child(
                        Button::new("save")
                            .primary()
                            .label("Save")
                            .on_click(cx.listener(|this, _, window, cx| this.submit(window, cx))),
                    ),
            )
    }
}
