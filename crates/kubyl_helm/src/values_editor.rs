//! The values editor of the install and upgrade dialogs: phase 04's YAML editor (gpui-component's
//! `EditorState`) with the chart's `values.schema.json` checked inline. The text stays in this
//! entity's memory; it reaches `helm` on stdin only.

use gpui::{
    AnyElement, App, AppContext as _, Context, Entity, IntoElement, Render, Subscription, Window,
    div, prelude::*,
};
use gpui_component::highlighter::{Diagnostic, DiagnosticSeverity};
use gpui_component::input::{Editor, EditorState, InputEvent, RopeExt as _};
use kubyl_helm_core::values;
use kubyl_ui::{ActiveColors, Icon, IconName, fonts, h_flex, u, v_flex};
use kubyl_yaml::validate::{Problem, Severity};
use serde_json::Value;

pub struct ValuesEditor {
    pub editor: Entity<EditorState>,
    /// The chart's schema, opened for JSON Schema's unknown keys.
    schema: Option<Value>,
    /// The text only overrides the defaults (required keys may be missing).
    partial: bool,
    problems: Vec<Problem>,
    _subscription: Subscription,
}

impl ValuesEditor {
    pub fn new(text: String, window: &mut Window, cx: &mut Context<Self>) -> Self {
        let editor = cx.new(|cx| {
            let mut state = EditorState::new(window, cx)
                .language("yaml")
                .line_number(true)
                .folding(true)
                .searchable(true)
                .soft_wrap(false);
            state.set_value(text, window, cx);
            state
        });
        let subscription = cx.subscribe(&editor, |this: &mut Self, _, event: &InputEvent, cx| {
            if matches!(event, InputEvent::Change) {
                this.check(cx);
            }
        });
        let mut this = Self {
            editor,
            schema: None,
            partial: false,
            problems: Vec::new(),
            _subscription: subscription,
        };
        this.check(cx);
        this
    }

    pub fn text(&self, cx: &App) -> String {
        self.editor.read(cx).value().to_string()
    }

    pub fn set_text(&mut self, text: String, window: &mut Window, cx: &mut Context<Self>) {
        self.editor
            .update(cx, |state, cx| state.set_value(text, window, cx));
        self.check(cx);
    }

    /// The chart's `values.schema.json` (`None`: no schema), and whether the text is
    /// override-only.
    pub fn set_schema(&mut self, schema: Option<Value>, partial: bool, cx: &mut Context<Self>) {
        self.schema = schema.map(|mut s| {
            values::open_schema(&mut s);
            s
        });
        self.partial = partial;
        self.check(cx);
    }

    pub fn has_schema(&self) -> bool {
        self.schema.is_some()
    }

    pub fn errors(&self) -> usize {
        self.problems
            .iter()
            .filter(|p| p.severity == Severity::Error)
            .count()
    }

    /// The values the text holds.
    pub fn values(&self, cx: &App) -> Result<Value, String> {
        values::parse(&self.text(cx))
    }

    fn check(&mut self, cx: &mut Context<Self>) {
        let text = self.text(cx);
        self.problems = values::problems(&text, self.schema.as_ref(), self.partial);
        let problems = self.problems.clone();
        self.editor.update(cx, |state, cx| {
            let rope = state.text().clone();
            let len = rope.len();
            if let Some(set) = state.diagnostics_mut() {
                set.reset(&rope);
                for p in problems {
                    let start = p.range.start.min(len);
                    let mut end = p.range.end.min(len);
                    if end <= start {
                        end = rope
                            .to_string()
                            .get(start..)
                            .and_then(|s| s.chars().next())
                            .map_or(start, |c| start + c.len_utf8());
                    }
                    let severity = match p.severity {
                        Severity::Error => DiagnosticSeverity::Error,
                        Severity::Warning => DiagnosticSeverity::Warning,
                        Severity::Info => DiagnosticSeverity::Info,
                    };
                    set.push(
                        Diagnostic::new(
                            rope.offset_to_position(start)..rope.offset_to_position(end),
                            p.message,
                        )
                        .with_severity(severity)
                        .with_source("values.schema.json"),
                    );
                }
            }
            cx.notify();
        });
        cx.notify();
    }

    /// The first problem, for the line under the editor.
    fn status(&self, cx: &App) -> Option<AnyElement> {
        let colors = cx.colors();
        let problem = self.problems.first()?;
        let more = self.problems.len() - 1;
        let path = problem
            .path
            .as_ref()
            .map(|p| format!("{p}: "))
            .unwrap_or_default();
        Some(
            h_flex()
                .flex_none()
                .gap(u(6.0))
                .px(u(10.0))
                .py(u(4.0))
                .border_t_1()
                .border_color(colors.border_variant)
                .text_size(u(11.5))
                .text_color(colors.red)
                .child(Icon::new(IconName::CircleX).size(12.0).color(colors.red))
                .child(
                    div()
                        .flex_1()
                        .min_w_0()
                        .truncate()
                        .child(format!("{path}{}", problem.message)),
                )
                .when(more > 0, |this| {
                    this.child(
                        div()
                            .text_color(colors.text_dim)
                            .child(format!("and {more} more")),
                    )
                })
                .into_any_element(),
        )
    }
}

impl Render for ValuesEditor {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let colors = cx.colors().clone();
        let status = self.status(cx);
        v_flex()
            .size_full()
            .rounded(u(6.0))
            .border_1()
            .border_color(colors.border)
            .bg(colors.background)
            .overflow_hidden()
            .child(
                div().flex_1().min_h_0().pt(u(4.0)).child(
                    Editor::new(&self.editor)
                        .h_full()
                        .bordered(false)
                        .font_family(fonts::MONO)
                        .text_size(u(12.5))
                        .bg(colors.background),
                ),
            )
            .children(status)
    }
}
