//! The query tool: a PromQL expression as a table (instant query) or a graph (range query), with
//! a history of this session. Expressions and results stay in memory, never on disk.

use std::collections::HashMap;
use std::ops::Range;
use std::sync::Arc;
use std::time::{Duration, Instant};

use gpui::{
    AnyElement, App, ClickEvent, Context, Entity, Focusable as _, IntoElement, SharedString,
    Subscription, UniformListScrollHandle, Window, div, prelude::*, uniform_list,
};
use gpui_component::button::Button as MenuButton;
use gpui_component::input::{Input, InputEvent, InputState};
use gpui_component::menu::{DropdownMenu as _, PopupMenuItem};
use kubyl_charts::data::{align, grid};
use kubyl_charts::{
    ChartData, ChartKind, LineChart, Series, TimeRange, TimeRangePicker, series_color,
};
use kubyl_core::{ColumnDef, ColumnWidth};
use kubyl_ui::{ActiveColors, Chip, Colors, Icon, IconName, fonts, h_flex, sizes, u, v_flex};

use super::{PrometheusView, widgets};
use crate::complete::{Completion, Kind, Request, complete_values, complete_with};
use crate::fetch::{self, Evaluation};
use crate::model::{QueryResult, RangeSeries, series_name};
use crate::service::Instance;

const ROW_HEIGHT: f32 = 29.0;
/// Series drawn in the graph; more are unreadable.
const MAX_SERIES: usize = 50;
const HISTORY: usize = 30;

/// Expressions to start from, for a server nobody has queried yet.
const EXAMPLES: [&str; 4] = [
    "up",
    "sum by (job) (up)",
    "rate(prometheus_http_requests_total[5m])",
    "topk(10, count by (__name__) ({__name__=~\".+\"}))",
];

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub(crate) enum Mode {
    #[default]
    Table,
    Graph,
}

/// A completion request's state: asked once, then kept.
enum Answer {
    Loading,
    Ready(Arc<Vec<String>>),
    /// Asked again after [`RETRY`].
    Failed(Instant),
}

/// How long a failed completion read waits before it is asked again.
const RETRY: Duration = Duration::from_secs(10);
/// Answers kept per server.
const MAX_ANSWERS: usize = 50;

pub(crate) struct Outcome {
    mode: Mode,
    result: Result<QueryResult, String>,
    took: Duration,
    /// `(series, value)` per row of the table.
    rows: Arc<Vec<(SharedString, SharedString)>>,
    /// Series the graph left out.
    hidden_series: usize,
}

impl Outcome {
    #[cfg(test)]
    pub(crate) fn for_test(result: QueryResult) -> Self {
        Self {
            mode: Mode::Table,
            rows: Arc::new(table_rows(&result)),
            result: Ok(result),
            took: Duration::from_millis(12),
            hidden_series: 0,
        }
    }
}

pub(crate) struct State {
    pub(super) input: Entity<InputState>,
    mode: Mode,
    range: TimeRange,
    running: bool,
    pub(super) outcome: Option<Outcome>,
    /// What the box offers for the word being typed, and which is highlighted.
    pub(super) suggestions: Option<Completion>,
    /// Answers of the server to completion requests, by [`Request::key`].
    answers: HashMap<String, Answer>,
    highlight: usize,
    /// The user moved the highlight: Enter takes it even where nothing is being typed.
    navigated: bool,
    /// The cursor the suggestions were made for.
    suggest_end: usize,
    history: Vec<String>,
    chart: Entity<LineChart>,
    scroll: UniformListScrollHandle,
    /// Counts runs: a result of an older run is dropped.
    run: u64,
}

impl State {
    pub(crate) fn new(window: &mut Window, cx: &mut Context<PrometheusView>) -> Self {
        Self {
            input: cx.new(|cx| {
                InputState::new(window, cx)
                    .placeholder("Expression, e.g. rate(http_requests_total[5m])")
            }),
            mode: Mode::Table,
            range: TimeRange::H1,
            running: false,
            outcome: None,
            suggestions: None,
            answers: HashMap::new(),
            highlight: 0,
            navigated: false,
            suggest_end: 0,
            history: Vec::new(),
            chart: cx.new(|_| {
                LineChart::new(ChartKind::Line, |v| widgets::format_value(&v.to_string()))
                    .with_height(280.0)
            }),
            scroll: UniformListScrollHandle::new(),
            run: 0,
        }
    }

    pub(crate) fn subscriptions(
        &self,
        window: &mut Window,
        cx: &mut Context<PrometheusView>,
    ) -> Vec<Subscription> {
        vec![cx.subscribe_in(
            &self.input,
            window,
            |this, _, event: &InputEvent, window, cx| match event {
                InputEvent::PressEnter { .. } => {
                    if !(this.enter_accepts(cx) && this.accept_suggestion(window, cx)) {
                        this.query_tab.suggestions = None;
                        this.run_query(window, cx);
                    }
                }
                InputEvent::Change => this.update_suggestions(cx),
                _ => {}
            },
        )]
    }

    /// The server changed: what was asked of the old one means nothing.
    pub(crate) fn reset(&mut self, cx: &mut Context<PrometheusView>) {
        self.run += 1;
        self.running = false;
        self.outcome = None;
        self.suggestions = None;
        self.answers.clear();
        self.chart
            .update(cx, |chart, cx| chart.set_data(ChartData::default(), cx));
    }
}

/// The rows of a result as the table shows them.
pub(crate) fn table_rows(result: &QueryResult) -> Vec<(SharedString, SharedString)> {
    match result {
        QueryResult::Vector(rows) => rows
            .iter()
            .map(|r| {
                (
                    series_name(&r.labels).into(),
                    widgets::format_value(&r.value).into(),
                )
            })
            .collect(),
        QueryResult::Matrix(series) => series
            .iter()
            .map(|s| {
                let last = s.points.last().map_or("—".to_string(), |(_, v)| {
                    widgets::format_value(&v.to_string())
                });
                (
                    series_name(&s.labels).into(),
                    format!("{last} ({} samples)", s.points.len()).into(),
                )
            })
            .collect(),
        QueryResult::Scalar(value) => vec![("scalar".into(), widgets::format_value(value).into())],
        QueryResult::String(value) => vec![("string".into(), value.clone().into())],
    }
}

/// The graph's data: every series on the grid of the query's window.
pub(crate) fn chart_data(
    series: &[RangeSeries],
    window: (f64, f64, f64),
    colors: &Colors,
) -> ChartData {
    let (start, end, step) = window;
    ChartData {
        times: grid(start, end, step),
        series: series
            .iter()
            .take(MAX_SERIES)
            .enumerate()
            .map(|(i, s)| {
                let name = series_name(&s.labels);
                Series::new(
                    name.clone(),
                    name,
                    series_color(i, colors),
                    align(&s.points, start, end, step),
                )
            })
            .collect(),
    }
}

fn columns() -> Vec<ColumnDef> {
    vec![
        ColumnDef::new(
            "series",
            "Series",
            ColumnWidth::Flex {
                weight: 1.0,
                min: 200.0,
            },
        )
        .mono(),
        ColumnDef::new("value", "Value", ColumnWidth::Fixed(220.0)).mono(),
    ]
}

impl PrometheusView {
    /// Runs the expression in the box as a table or a graph, per the mode.
    pub(crate) fn run_query(&mut self, _: &mut Window, cx: &mut Context<Self>) {
        let expression = self.query_tab.input.read(cx).value().trim().to_string();
        if expression.is_empty() {
            return;
        }
        let Some(instance) = self.current(cx) else {
            return;
        };
        let history = &mut self.query_tab.history;
        history.retain(|e| *e != expression);
        history.insert(0, expression.clone());
        history.truncate(HISTORY);
        let mode = self.query_tab.mode;
        let (evaluation, window) = match mode {
            Mode::Table => (Evaluation::Instant { time: None }, None),
            Mode::Graph => {
                let window = self.query_tab.range.window(kubyl_metrics::service::now());
                (
                    Evaluation::Range {
                        start: window.0,
                        end: window.1,
                        step: window.2,
                    },
                    Some(window),
                )
            }
        };
        self.query_tab.running = true;
        self.query_tab.run += 1;
        let run = self.query_tab.run;
        let prom = instance.client.clone();
        let started = Instant::now();
        self.spawn_read(
            cx,
            async move {
                fetch::query(&prom, &expression, evaluation)
                    .await
                    .map_err(|e| e.to_string())
            },
            move |this, result, cx| {
                if this.query_tab.run != run {
                    return;
                }
                let colors = cx.colors().clone();
                let mut hidden_series = 0;
                let mut rows = Vec::new();
                match (&result, window) {
                    (Ok(QueryResult::Matrix(series)), Some(window)) => {
                        hidden_series = series.len().saturating_sub(MAX_SERIES);
                        let data = chart_data(series, window, &colors);
                        let chart = this.query_tab.chart.clone();
                        chart.update(cx, |chart, cx| {
                            chart.set_placeholder("No data in this range.", cx);
                            chart.set_data(data, cx)
                        });
                    }
                    (Ok(result), _) => rows = table_rows(result),
                    (Err(_), _) => {}
                }
                this.query_tab.running = false;
                this.query_tab.outcome = Some(Outcome {
                    mode,
                    result,
                    took: started.elapsed(),
                    rows: Arc::new(rows),
                    hidden_series,
                });
            },
        );
        cx.notify();
    }

    /// Completes the word at the end of the box.
    pub(crate) fn update_suggestions(&mut self, cx: &mut Context<Self>) {
        self.suggest(false, cx);
    }

    /// [`Self::update_suggestions`]; `explicit` (Ctrl+Space) asks for everything that fits.
    pub(crate) fn suggest(&mut self, explicit: bool, cx: &mut Context<Self>) {
        // Completion works on what is left of the cursor.
        let (text, cursor) = {
            let input = self.query_tab.input.read(cx);
            let value = input.value();
            let cursor = input.cursor().min(value.len());
            (value[..cursor].to_string(), cursor)
        };
        self.query_tab.suggest_end = cursor;
        let empty = crate::complete::Names::default();
        let loaded = self.names.data.clone();
        let names = loaded.as_deref().unwrap_or(&empty);
        self.query_tab.suggestions = match crate::complete::request(&text) {
            Some((request, start)) => match self.answer(&request, cx) {
                Some(values) => match &request {
                    Request::Values { .. } => complete_values(&text, start, &values, 10),
                    Request::Labels { .. } => complete_with(
                        &text,
                        &crate::complete::Names {
                            labels: values.to_vec(),
                            ..Default::default()
                        },
                        10,
                        explicit,
                    ),
                },
                // Not here yet: label names fall back to all of them, values show when they come.
                None => match request {
                    Request::Labels { .. } => complete_with(&text, names, 10, explicit),
                    Request::Values { .. } => None,
                },
            },
            None => complete_with(&text, names, 10, explicit),
        };
        self.query_tab.highlight = 0;
        self.query_tab.navigated = false;
        cx.notify();
    }

    /// The server's answer to a completion request. The first ask starts the read and the
    /// answer (or its failure) is kept for this server.
    fn answer(&mut self, request: &Request, cx: &mut Context<Self>) -> Option<Arc<Vec<String>>> {
        let key = request.key();
        match self.query_tab.answers.get(&key) {
            Some(Answer::Ready(values)) => return Some(values.clone()),
            Some(Answer::Loading) => return None,
            Some(Answer::Failed(at)) if at.elapsed() < RETRY => return None,
            Some(Answer::Failed(_)) | None => {}
        }
        if self.query_tab.answers.len() >= MAX_ANSWERS {
            self.query_tab.answers.clear();
        }
        let instance = self.current(cx)?;
        self.query_tab.answers.insert(key.clone(), Answer::Loading);
        let prom = instance.client.clone();
        let request = request.clone();
        self.spawn_read(
            cx,
            async move {
                match &request {
                    Request::Values { label, selector } => {
                        fetch::label_values(&prom, label, selector.as_deref()).await
                    }
                    Request::Labels { selector } => fetch::labels_of(&prom, selector).await,
                }
            },
            move |this, result, cx| {
                let answer = match result {
                    Ok(values) => Answer::Ready(Arc::new(values)),
                    Err(_) => Answer::Failed(Instant::now()),
                };
                this.query_tab.answers.insert(key, answer);
                // The box may have moved on: complete what it holds now.
                this.update_suggestions(cx);
            },
        );
        None
    }

    pub(crate) fn move_suggestion(&mut self, delta: isize, cx: &mut Context<Self>) {
        let Some(suggestions) = &self.query_tab.suggestions else {
            return;
        };
        let last = suggestions.items.len() as isize - 1;
        self.query_tab.highlight =
            (self.query_tab.highlight as isize + delta).clamp(0, last) as usize;
        self.query_tab.navigated = true;
        cx.notify();
    }

    /// Whether Enter takes the highlighted completion: one is open and it adds something. When
    /// the box already holds exactly that word, Enter runs the query.
    pub(crate) fn enter_accepts(&self, cx: &Context<Self>) -> bool {
        let Some(suggestions) = &self.query_tab.suggestions else {
            return false;
        };
        let Some(item) = suggestions.items.get(self.query_tab.highlight) else {
            return false;
        };
        let text = self.query_tab.input.read(cx).value();
        let typed = text.get(suggestions.start..self.query_tab.suggest_end);
        // Nothing typed (after an operand or `(`): Enter runs the query unless the user picked.
        let adds = typed != Some(item.insert.as_str());
        adds && (self.query_tab.navigated || typed.is_some_and(|t| !t.is_empty()))
    }

    /// Takes the highlighted completion; `false` when there is none. It replaces the word
    /// around the cursor and leaves the cursor inside a snippet's brackets.
    pub(crate) fn accept_suggestion(
        &mut self,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> bool {
        let Some(suggestions) = self.query_tab.suggestions.take() else {
            return false;
        };
        let Some(item) = suggestions.items.get(self.query_tab.highlight) else {
            return false;
        };
        let text = self.query_tab.input.read(cx).value().to_string();
        let start = suggestions.start.min(text.len());
        let mut end = self.query_tab.suggest_end.clamp(start, text.len());
        // The rest of a word the cursor is in goes too. An item that closes a string takes the
        // rest of that string, up to its closing quote.
        if item.insert.ends_with('"') {
            let mut escaped = false;
            for (i, c) in text[end..].char_indices() {
                if escaped {
                    escaped = false;
                } else if c == '\\' {
                    escaped = true;
                } else if c == '"' {
                    end += i + 1;
                    break;
                }
            }
        } else {
            end += text[end..]
                .chars()
                .take_while(|c| c.is_ascii_alphanumeric() || *c == '_' || *c == ':')
                .map(char::len_utf8)
                .sum::<usize>();
        }
        let next = format!("{}{}{}", &text[..start], item.insert, &text[end..]);
        let cursor = start + item.cursor.unwrap_or(item.insert.len());
        let column = next[..cursor].encode_utf16().count() as u32;
        self.query_tab.input.update(cx, |input, cx| {
            input.set_value(next, window, cx);
            input.set_cursor_position(gpui_component::input::Position::new(0, column), window, cx);
        });
        // `set_value` reports no change: the box holds new text now.
        self.update_suggestions(cx);
        true
    }

    /// Puts `expression` in the box and runs it.
    fn use_expression(&mut self, expression: String, window: &mut Window, cx: &mut Context<Self>) {
        self.query_tab
            .input
            .update(cx, |input, cx| input.set_value(expression, window, cx));
        self.query_tab.suggestions = None;
        self.run_query(window, cx);
    }

    pub(super) fn render_query(
        &mut self,
        _instance: &Instance,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let colors = cx.colors().clone();
        let focused = self
            .query_tab
            .input
            .read(cx)
            .focus_handle(cx)
            .is_focused(window);
        let mode = self.query_tab.mode;
        let weak = cx.weak_entity();
        let history = self.query_tab.history.clone();
        let history_menu = (!history.is_empty()).then(|| {
            MenuButton::new("query-history")
                .outline()
                .compact()
                .child(
                    h_flex()
                        .gap(u(6.0))
                        .text_size(u(12.0))
                        .child(Icon::new(IconName::History).size(12.0))
                        .child("History"),
                )
                .dropdown_menu(move |mut menu, _, _| {
                    menu = menu.max_h(gpui::px(420.0)).scrollable(true);
                    for expression in &history {
                        let weak = weak.clone();
                        let pick = expression.clone();
                        let label: String = expression.chars().take(90).collect();
                        menu = menu.item(PopupMenuItem::new(label).on_click(
                            move |_: &ClickEvent, window: &mut Window, cx: &mut App| {
                                let pick = pick.clone();
                                weak.update(cx, |this, cx| this.use_expression(pick, window, cx))
                                    .ok();
                            },
                        ));
                    }
                    menu
                })
        });
        let input_row = h_flex()
            .flex_none()
            .px(u(12.0))
            .py(u(10.0))
            .gap(u(8.0))
            .child(
                div()
                    .flex_1()
                    .min_w_0()
                    .h(u(sizes::CONTROL + 4.0))
                    .px(u(10.0))
                    .flex()
                    .items_center()
                    .rounded(u(5.0))
                    .bg(colors.input_background)
                    .border_1()
                    .border_color(if focused {
                        colors.accent
                    } else {
                        colors.border
                    })
                    .font_family(fonts::MONO)
                    .child(
                        div().flex_1().min_w_0().child(
                            Input::new(&self.query_tab.input)
                                .appearance(false)
                                .text_size(u(12.5)),
                        ),
                    ),
            )
            .child(widgets::button(
                "query-execute",
                Some(IconName::Play),
                if self.query_tab.running {
                    "Running…"
                } else {
                    "Execute"
                },
                true,
                &colors,
                cx.listener(|this, _: &ClickEvent, window, cx| this.run_query(window, cx)),
            ))
            .children(history_menu);
        let mode_chip =
            |id: &'static str, label: &'static str, value: Mode, cx: &mut Context<Self>| {
                div()
                    .id(id)
                    .cursor_pointer()
                    .child(Chip::new(label).selected(mode == value))
                    .on_click(cx.listener(move |this, _, window, cx| {
                        if this.query_tab.mode != value {
                            this.query_tab.mode = value;
                            // The result belongs to the other mode: ask again.
                            this.run_query(window, cx);
                        }
                        cx.notify();
                    }))
            };
        let stats: Option<String> = self
            .query_tab
            .outcome
            .as_ref()
            .and_then(|o| match &o.result {
                Ok(result) => Some(format!(
                    "{} {} · {} ms",
                    result.len(),
                    if result.len() == 1 { "row" } else { "rows" },
                    o.took.as_millis()
                )),
                Err(_) => None,
            });
        let options_row = widgets::toolbar(&colors)
            .child(mode_chip("query-table", "Table", Mode::Table, cx))
            .child(mode_chip("query-graph", "Graph", Mode::Graph, cx))
            .when(mode == Mode::Graph, |this| {
                this.child(
                    TimeRangePicker::new("query-range", self.query_tab.range).on_change({
                        let weak = cx.weak_entity();
                        move |range, window, cx| {
                            weak.update(cx, |this, cx| {
                                this.query_tab.range = range;
                                this.run_query(window, cx);
                            })
                            .ok();
                        }
                    }),
                )
            })
            .child(div().flex_1())
            .children(stats.map(|s| div().text_color(colors.text_dim).child(s)))
            .child(
                div()
                    .text_color(colors.text_dim)
                    .child(if cfg!(target_os = "macos") {
                        "⌘↵ to run"
                    } else {
                        "Ctrl+↵ to run"
                    }),
            );
        let result = self.render_query_result(window, cx);
        let suggestions = focused
            .then(|| self.render_suggestions(&colors, cx))
            .flatten()
            // Deferred: painted over the result below.
            .map(|list| gpui::deferred(list).with_priority(1));
        v_flex()
            .relative()
            .size_full()
            .children(suggestions)
            .child(input_row)
            .child(options_row)
            .child(div().flex().flex_1().min_h_0().child(result))
            .into_any_element()
    }

    fn render_suggestions(&self, colors: &Colors, cx: &mut Context<Self>) -> Option<AnyElement> {
        let suggestions = self.query_tab.suggestions.as_ref()?;
        Some(
            v_flex()
                .id("query-suggestions")
                .occlude()
                .absolute()
                .top(u(sizes::CONTROL + 20.0))
                .left(u(12.0))
                .w(u(620.0))
                .p(u(4.0))
                .rounded(u(7.0))
                .bg(colors.elevated)
                .border_1()
                .border_color(colors.border)
                .shadow_lg()
                .text_size(u(12.0))
                .children(suggestions.items.iter().enumerate().map(|(i, item)| {
                    let highlighted = i == self.query_tab.highlight;
                    let kind_color = match item.kind {
                        Kind::Metric => colors.accent,
                        Kind::Function => colors.yellow,
                        Kind::Keyword | Kind::Label | Kind::Operator | Kind::Unit => {
                            colors.text_dim
                        }
                        Kind::Value => colors.green,
                        Kind::Snippet => colors.accent,
                    };
                    h_flex()
                        .id(("query-suggestion", i))
                        .px(u(8.0))
                        .py(u(4.0))
                        .gap(u(8.0))
                        .rounded(u(4.0))
                        .cursor_pointer()
                        .when(highlighted, |this| this.bg(colors.chip_selected_background))
                        .hover(|s| s.bg(colors.hover))
                        .child(
                            div()
                                .when(item.detail.is_none(), |this| this.flex_1())
                                .when(item.detail.is_some(), |this| {
                                    this.flex_none().max_w(u(280.0))
                                })
                                .min_w_0()
                                .truncate()
                                .font_family(fonts::MONO)
                                .child(item.label.clone()),
                        )
                        .children(item.detail.clone().map(|detail| {
                            div()
                                .flex_1()
                                .min_w_0()
                                .truncate()
                                .text_size(u(11.0))
                                .text_color(colors.text_dim)
                                .child(detail)
                        }))
                        .child(
                            div()
                                .flex_none()
                                .text_size(u(11.0))
                                .text_color(kind_color)
                                .child(item.kind.label()),
                        )
                        .on_click(cx.listener(move |this, _, window, cx| {
                            this.query_tab.highlight = i;
                            this.accept_suggestion(window, cx);
                            this.query_tab
                                .input
                                .read(cx)
                                .focus_handle(cx)
                                .focus(window, cx);
                        }))
                }))
                .child(
                    div()
                        .px(u(8.0))
                        .pt(u(5.0))
                        .mt(u(3.0))
                        .border_t_1()
                        .border_color(colors.border_variant)
                        .text_size(u(11.0))
                        .text_color(colors.text_dim)
                        .child("↑↓ choose · Tab accept · Esc close"),
                )
                .into_any_element(),
        )
    }

    fn render_query_result(&mut self, _: &mut Window, cx: &mut Context<Self>) -> AnyElement {
        let colors = cx.colors().clone();
        let Some(outcome) = &self.query_tab.outcome else {
            return v_flex()
                .flex_1()
                .items_center()
                .justify_center()
                .gap(u(10.0))
                .child(
                    div()
                        .text_size(u(12.5))
                        .text_color(colors.text_dim)
                        .child("Enter a PromQL expression and press Execute. Try one of these:"),
                )
                .children(EXAMPLES.iter().enumerate().map(|(i, example)| {
                    let example = example.to_string();
                    div()
                        .id(("query-example", i))
                        .cursor_pointer()
                        .child(Chip::new(example.clone()).mono())
                        .on_click(cx.listener(move |this, _, window, cx| {
                            this.use_expression(example.clone(), window, cx)
                        }))
                }))
                .into_any_element();
        };
        match (&outcome.result, outcome.mode) {
            (Err(error), _) => v_flex()
                .flex_1()
                .p(u(14.0))
                .child(widgets::code(error.clone(), &colors).text_color(colors.red))
                .into_any_element(),
            (Ok(result), _) if result.is_empty() => widgets::empty("Empty query result.", &colors),
            (Ok(_), Mode::Graph) => {
                let hidden = outcome.hidden_series;
                v_flex()
                    .id("query-graph-area")
                    .flex_1()
                    .min_w_0()
                    .overflow_y_scroll()
                    .p(u(14.0))
                    .gap(u(8.0))
                    .child(self.query_tab.chart.clone())
                    .when(hidden > 0, |this| {
                        this.child(
                            div()
                                .text_size(u(11.5))
                                .text_color(colors.text_dim)
                                .child(format!(
                                    "Showing {MAX_SERIES} of {} series. Narrow the expression to see the rest.",
                                    MAX_SERIES + hidden
                                )),
                        )
                    })
                    .into_any_element()
            }
            (Ok(_), Mode::Table) => {
                let count = outcome.rows.len();
                v_flex()
                    .flex_1()
                    .min_h_0()
                    .child(widgets::header(&columns(), &colors))
                    .child(
                        uniform_list(
                            "query-rows",
                            count,
                            cx.processor(|this, range: Range<usize>, _, cx| {
                                this.render_result_rows(range, cx)
                            }),
                        )
                        .flex_1()
                        .track_scroll(&self.query_tab.scroll),
                    )
                    .into_any_element()
            }
        }
    }

    fn render_result_rows(
        &mut self,
        range: Range<usize>,
        cx: &mut Context<Self>,
    ) -> Vec<AnyElement> {
        let colors = cx.colors().clone();
        let cols = columns();
        let Some(outcome) = &self.query_tab.outcome else {
            return Vec::new();
        };
        range
            .filter_map(|index| {
                let (series, value) = outcome.rows.get(index)?.clone();
                let copy = series.to_string();
                Some(
                    widgets::row(("query-row", index), false, ROW_HEIGHT, &colors)
                        .children(cols.iter().zip([series, value]).map(|(def, text)| {
                            widgets::column_cell(def)
                                .font_family(fonts::MONO)
                                .text_size(u(11.5))
                                .truncate()
                                .child(text)
                        }))
                        .on_click(move |_, _, cx| widgets::copy(copy.clone(), "the series", cx))
                        .into_any_element(),
                )
            })
            .collect()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::{InstantSample, Labels};

    fn labels(pairs: &[(&str, &str)]) -> Labels {
        pairs
            .iter()
            .map(|(k, v)| (k.to_string(), v.to_string()))
            .collect()
    }

    #[test]
    fn vector_rows() {
        let rows = table_rows(&QueryResult::Vector(vec![InstantSample {
            labels: labels(&[("__name__", "up"), ("job", "a")]),
            value: "1".into(),
        }]));
        assert_eq!(rows[0].0.as_ref(), r#"up{job="a"}"#);
        assert_eq!(rows[0].1.as_ref(), "1");
        assert_eq!(
            table_rows(&QueryResult::Scalar("3".into()))[0].0.as_ref(),
            "scalar"
        );
    }

    #[test]
    fn matrix_rows_show_the_last_value() {
        let rows = table_rows(&QueryResult::Matrix(vec![RangeSeries {
            labels: labels(&[("__name__", "x")]),
            points: vec![(1.0, 1.0), (2.0, 2.5)],
        }]));
        assert_eq!(rows[0].1.as_ref(), "2.5 (2 samples)");
    }

    #[test]
    fn graph_series_are_aligned_to_the_window() {
        let colors = Colors::one_dark();
        let series = vec![RangeSeries {
            labels: labels(&[("__name__", "x")]),
            points: vec![(0.0, 1.0), (30.0, 3.0)],
        }];
        let data = chart_data(&series, (0.0, 60.0, 15.0), &colors);
        assert_eq!(data.times, [0.0, 15.0, 30.0, 45.0, 60.0]);
        assert_eq!(
            data.series[0].values,
            [Some(1.0), None, Some(3.0), None, None],
            "gaps stay gaps"
        );
    }

    #[test]
    fn graphs_cap_the_series() {
        let colors = Colors::one_dark();
        let series: Vec<RangeSeries> = (0..MAX_SERIES + 5)
            .map(|i| RangeSeries {
                labels: labels(&[("i", &i.to_string())]),
                points: vec![(0.0, 1.0)],
            })
            .collect();
        assert_eq!(
            chart_data(&series, (0.0, 15.0, 15.0), &colors).series.len(),
            MAX_SERIES
        );
    }
}
