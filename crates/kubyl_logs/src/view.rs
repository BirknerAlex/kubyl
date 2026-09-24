//! `ViewKind::Logs`: the log view (board 2 · Live logs).
//!
//! Renders a virtualized list of ring-buffer lines with level chips, text/regex search and
//! pause/follow controls. Streaming itself lives in [`crate::stream`]; this module owns the
//! ring buffer, search/filter state and the kube object lookups needed to resolve a workload's
//! pod selector.

use std::ops::Range;

use futures::StreamExt as _;
use futures::channel::mpsc;
use gpui::{
    App, AppContext as _, Context, Entity, FocusHandle, Focusable, IntoElement, SharedString,
    Subscription, Task, Window, actions, div, prelude::*, uniform_list,
};
use gpui_component::input::{Input, InputEvent, InputState};
use k8s_openapi::api::apps::v1::{DaemonSet, Deployment, StatefulSet};
use k8s_openapi::api::batch::v1::Job;
use kube::Api;
use kubyl_core::{ClusterId, ResourceRef, TabView, Tone, ViewRequest};
use kubyl_kube::ConnectionManager;
use kubyl_ui::{ActiveColors, Colors, Icon, IconName, fonts, h_flex, u, v_flex};

use crate::json;
use crate::level::LogLevel;
use crate::line::{LogLine, pod_color};
use crate::ring::LogRingBuffer;
use crate::search::{MatchCursor, Search};
use crate::sessions::{SessionId, SessionKind, SessionRegistry};
use crate::settings::LogsSettings;
use crate::stream::{LogOptions, LogSource, StreamEvent};

pub(crate) const CONTEXT: &str = "LogsView";

actions!(
    logs,
    [
        TogglePause,
        JumpToBottom,
        ToggleWrap,
        ToggleTimestamps,
        ToggleFollow,
        TogglePretty,
        ToggleRegex,
        ToggleCase,
        FilterToMatches,
        NextMatch,
        PrevMatch,
        CopyVisible,
    ]
);

pub struct LogsView {
    focus: FocusHandle,
    request: ViewRequest,
    target: ResourceRef,
    kind: String,
    ring: LogRingBuffer,
    paused: bool,
    paused_new_lines: u64,
    wrap: bool,
    pretty_json: bool,
    follow: bool,
    active_levels: [bool; LogLevel::ALL.len()],
    search_input: Entity<InputState>,
    search: Option<Search>,
    search_regex: bool,
    search_case_sensitive: bool,
    filter_to_matches: bool,
    cursor: MatchCursor,
    status: SharedString,
    session_id: Option<SessionId>,
    _stream_task: Option<Task<()>>,
    _subscriptions: Vec<Subscription>,
}

impl LogsView {
    pub fn new(target: Option<ResourceRef>, window: &mut Window, cx: &mut Context<Self>) -> Self {
        let target = target.unwrap_or_else(|| {
            ResourceRef::object(
                ClusterId::new(""),
                kubyl_core::Gvr::new("", "v1", "pods"),
                None,
                String::new(),
            )
        });
        let kind = kind_label(&target.gvr.resource);
        let search_input =
            cx.new(|cx| InputState::new(window, cx).placeholder("Search (text or /regex/)"));
        let subscription = cx.subscribe_in(
            &search_input,
            window,
            |this, input, event: &InputEvent, window, cx| {
                if matches!(event, InputEvent::Change | InputEvent::PressEnter { .. }) {
                    let _ = window;
                    let text = input.read(cx).value().to_string();
                    this.set_query(text, cx);
                }
            },
        );

        let capacity = kubyl_settings::Settings::get::<LogsSettings>(cx).ring_buffer_lines;
        let settings = kubyl_settings::Settings::get::<LogsSettings>(cx).clone();
        let mut this = Self {
            focus: cx.focus_handle(),
            request: ViewRequest::for_resource(kubyl_core::ViewKind::Logs, target.clone()),
            target,
            kind,
            ring: LogRingBuffer::new(capacity),
            paused: false,
            paused_new_lines: 0,
            wrap: settings.wrap,
            pretty_json: false,
            follow: settings.follow,
            active_levels: [true; LogLevel::ALL.len()],
            search_input,
            search: None,
            search_regex: false,
            search_case_sensitive: false,
            filter_to_matches: false,
            cursor: MatchCursor::default(),
            status: "connecting…".into(),
            session_id: None,
            _stream_task: None,
            _subscriptions: vec![subscription],
        };
        this.start(cx);
        this
    }

    fn start(&mut self, cx: &mut Context<Self>) {
        let Some(client) = ConnectionManager::global(cx)
            .read(cx)
            .client(&self.target.cluster)
        else {
            self.status = "not connected".into();
            return;
        };
        let namespace = self.target.namespace.clone().unwrap_or_default();
        let resource = self.target.gvr.resource.clone();
        let name = self.target.name.clone().unwrap_or_default();
        let settings = kubyl_settings::Settings::get::<LogsSettings>(cx);
        let options = LogOptions {
            follow: self.follow,
            tail_lines: Some(settings.tail_lines),
            timestamps: settings.timestamps,
            ..Default::default()
        };
        self.wrap = settings.wrap;

        let (tx, rx) = mpsc::unbounded();
        let source_client = client.clone();
        let title = format!("{} {}", self.kind, name);
        let source_task = kubyl_core::spawn_kube(cx, async move {
            resolve_source(source_client, &namespace, &resource, &name).await
        });
        let subtitle: SharedString = self.target.namespace.clone().unwrap_or_default().into();
        self._stream_task = Some(cx.spawn(async move |this, cx| {
            let source = match source_task.await {
                Ok(source) => source,
                Err(err) => {
                    this.update(cx, |this, cx| {
                        this.status = format!("error: {err}").into();
                        cx.notify();
                    })
                    .ok();
                    return;
                }
            };
            let namespace = this
                .read_with(cx, |this, _| {
                    this.target.namespace.clone().unwrap_or_default()
                })
                .unwrap_or_default();
            let run_client = client.clone();
            // Kept alive for as long as this loop runs; dropping it aborts the stream (the
            // Task-drop rule: `spawn_kube` tasks stop their Tokio work when dropped).
            let _run_task = cx.update(|cx| {
                kubyl_core::spawn_kube(cx, async move {
                    crate::stream::run(run_client, namespace, source, options, tx).await
                })
            });
            this.update(cx, |this, cx| {
                let id = SessionRegistry::add(
                    cx,
                    SessionKind::Logs,
                    title,
                    subtitle,
                    "streaming",
                    Tone::Good,
                    |_| {},
                );
                this.session_id = Some(id);
            })
            .ok();
            let mut rx = rx;
            while let Some(first) = rx.next().await {
                let mut batch = vec![first];
                while let Ok(event) = rx.try_recv() {
                    batch.push(event);
                    if batch.len() > 5000 {
                        break;
                    }
                }
                let alive = this
                    .update(cx, |this, cx| this.apply_events(batch, cx))
                    .is_ok();
                if !alive {
                    break;
                }
                // Batches updates at ~60 Hz so high-volume streams don't re-render per line.
                cx.background_executor()
                    .timer(std::time::Duration::from_millis(16))
                    .await;
            }
        }));
    }

    fn apply_events(&mut self, events: Vec<StreamEvent>, cx: &mut Context<Self>) {
        let mut appended = 0u64;
        for event in events {
            match event {
                StreamEvent::Line {
                    pod,
                    container,
                    text,
                    ..
                } => {
                    self.ring.push(pod, container, text);
                    appended += 1;
                }
                StreamEvent::PodJoined(pod) => {
                    self.ring
                        .push_gap(pod.clone(), String::new(), format!("── {pod} joined ──"));
                }
                StreamEvent::PodGone(pod) => {
                    self.ring
                        .push_gap(pod.clone(), String::new(), format!("── {pod} left ──"));
                }
                StreamEvent::Reconnecting { pod, container } => {
                    self.ring
                        .push_gap(pod, container, "── reconnecting… ──".to_string());
                    if let Some(id) = self.session_id {
                        SessionRegistry::set_status(cx, id, "reconnecting", Tone::Warning);
                    }
                }
                StreamEvent::Error(message) => {
                    self.status = format!("error: {message}").into();
                }
            }
        }
        if self.paused {
            self.paused_new_lines += appended;
        } else if let Some(id) = self.session_id {
            SessionRegistry::set_status(cx, id, "streaming", Tone::Good);
        }
        self.recompute_search();
        cx.notify();
    }

    fn set_query(&mut self, query: String, cx: &mut Context<Self>) {
        if query.is_empty() {
            self.search = None;
            self.cursor.set_matches(Vec::new());
        } else {
            match Search::new(&query, self.search_regex, self.search_case_sensitive) {
                Ok(search) => self.search = Some(search),
                Err(err) => self.status = format!("bad pattern: {err}").into(),
            }
        }
        self.recompute_search();
        cx.notify();
    }

    fn recompute_search(&mut self) {
        let Some(search) = &self.search else {
            self.cursor.set_matches(Vec::new());
            return;
        };
        let visible: Vec<&LogLine> = self.filtered_lines();
        let matches = search.find_all(&visible);
        self.cursor.set_matches(matches);
    }

    /// Lines passing the level filter, in ring-buffer order.
    fn filtered_lines(&self) -> Vec<&LogLine> {
        self.ring
            .iter()
            .filter(|line| line.gap_marker || self.active_levels[level_index(line.level)])
            .collect()
    }

    fn rendered_lines(&self) -> Vec<&LogLine> {
        let base = self.filtered_lines();
        if self.filter_to_matches
            && let Some(search) = &self.search
        {
            base.into_iter()
                .filter(|l| search.matches(&l.text))
                .collect()
        } else {
            base
        }
    }

    fn level_counts(&self) -> [usize; LogLevel::ALL.len()] {
        let mut counts = [0usize; LogLevel::ALL.len()];
        for line in self.ring.iter() {
            if !line.gap_marker {
                counts[level_index(line.level)] += 1;
            }
        }
        counts
    }

    fn toggle_level(&mut self, level: LogLevel, cx: &mut Context<Self>) {
        self.active_levels[level_index(level)] ^= true;
        self.recompute_search();
        cx.notify();
    }

    fn toggle_pause(&mut self, cx: &mut Context<Self>) {
        self.paused = !self.paused;
        if !self.paused {
            self.paused_new_lines = 0;
        }
        cx.notify();
    }

    fn copy_visible(&mut self, cx: &mut Context<Self>) {
        let text = self
            .rendered_lines()
            .iter()
            .map(|l| l.text.as_str())
            .collect::<Vec<_>>()
            .join("\n");
        cx.write_to_clipboard(gpui::ClipboardItem::new_string(text));
    }

    fn render_line(&self, line: &LogLine, colors: &Colors) -> impl IntoElement {
        let color = pod_color(line.pod_color_index, colors);
        let display_text = if self.pretty_json
            && let Some(value) = json::parse_object(&line.text)
        {
            json::pretty(&value)
        } else {
            line.text.clone()
        };
        let level_color = match line.level {
            LogLevel::Error | LogLevel::Fatal => colors.red,
            LogLevel::Warn => colors.yellow,
            LogLevel::Debug | LogLevel::Trace => colors.text_faint,
            _ => colors.text,
        };
        h_flex()
            .id(("log-line", line.seq as usize))
            .gap(u(6.0))
            .px(u(8.0))
            .when(line.gap_marker, |this| {
                this.text_color(colors.text_faint).italic()
            })
            .when(!line.gap_marker, |this| {
                this.child(
                    div()
                        .w(u(3.0))
                        .h(u(14.0))
                        .rounded(u(1.0))
                        .bg(color)
                        .flex_none(),
                )
                .child(
                    div()
                        .flex_none()
                        .w(u(120.0))
                        .text_color(colors.text_dim)
                        .child(format!("{}/{}", line.pod, line.container)),
                )
                .text_color(level_color)
            })
            .child(
                div()
                    .font_family(fonts::MONO)
                    .text_size(u(12.0))
                    .when(self.wrap, |this| this.whitespace_normal())
                    .when(!self.wrap, |this| this.whitespace_nowrap())
                    .child(display_text),
            )
    }
}

fn level_index(level: LogLevel) -> usize {
    LogLevel::ALL.iter().position(|l| *l == level).unwrap_or(0)
}

fn kind_label(resource: &str) -> String {
    resource.strip_suffix('s').unwrap_or(resource).to_string()
}

/// Resolves a `ResourceRef` into a [`LogSource`]: pods stream themselves, workloads resolve
/// their pod-template label selector once and stream every matching pod.
async fn resolve_source(
    client: kube::Client,
    namespace: &str,
    resource: &str,
    name: &str,
) -> anyhow::Result<LogSource> {
    match resource {
        "pods" => Ok(LogSource::Pod {
            pod: name.to_string(),
            init_containers: false,
        }),
        "deployments" => {
            let api: Api<Deployment> = Api::namespaced(client, namespace);
            let obj = api.get(name).await?;
            let selector = obj
                .spec
                .and_then(|s| s.selector.match_labels)
                .unwrap_or_default();
            Ok(LogSource::Workload {
                label_selector: to_selector(selector),
            })
        }
        "statefulsets" => {
            let api: Api<StatefulSet> = Api::namespaced(client, namespace);
            let obj = api.get(name).await?;
            let selector = obj
                .spec
                .and_then(|s| s.selector.match_labels)
                .unwrap_or_default();
            Ok(LogSource::Workload {
                label_selector: to_selector(selector),
            })
        }
        "daemonsets" => {
            let api: Api<DaemonSet> = Api::namespaced(client, namespace);
            let obj = api.get(name).await?;
            let selector = obj
                .spec
                .and_then(|s| s.selector.match_labels)
                .unwrap_or_default();
            Ok(LogSource::Workload {
                label_selector: to_selector(selector),
            })
        }
        "jobs" => {
            let api: Api<Job> = Api::namespaced(client, namespace);
            let obj = api.get(name).await?;
            let selector = obj
                .spec
                .and_then(|s| s.selector)
                .and_then(|s| s.match_labels)
                .unwrap_or_default();
            Ok(LogSource::Workload {
                label_selector: to_selector(selector),
            })
        }
        other => anyhow::bail!("logs aren't supported for {other}"),
    }
}

fn to_selector(labels: std::collections::BTreeMap<String, String>) -> String {
    labels
        .into_iter()
        .map(|(k, v)| format!("{k}={v}"))
        .collect::<Vec<_>>()
        .join(",")
}

impl Focusable for LogsView {
    fn focus_handle(&self, _: &App) -> FocusHandle {
        self.focus.clone()
    }
}

impl TabView for LogsView {
    fn tab_title(&self, _: &App) -> SharedString {
        format!("Logs: {}", self.target.name.clone().unwrap_or_default()).into()
    }

    fn tab_icon(&self, _: &App) -> Option<SharedString> {
        Some(IconName::Terminal.path())
    }

    fn view_request(&self, _: &App) -> Option<ViewRequest> {
        Some(self.request.clone())
    }
}

impl Render for LogsView {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let colors = cx.colors().clone();
        let counts = self.level_counts();
        let row_count = self.rendered_lines().len();
        let status = self.status.clone();
        let match_label: SharedString = match (&self.search, self.cursor.position()) {
            (Some(_), Some(pos)) => format!("{pos} / {}", self.cursor.count()).into(),
            (Some(_), None) => "0 matches".into(),
            (None, _) => SharedString::default(),
        };

        v_flex()
            .key_context(CONTEXT)
            .track_focus(&self.focus)
            .size_full()
            .bg(colors.background)
            .text_color(colors.text)
            .on_action(cx.listener(|this, _: &TogglePause, _, cx| this.toggle_pause(cx)))
            .on_action(cx.listener(|this, _: &ToggleWrap, _, cx| {
                this.wrap = !this.wrap;
                cx.notify();
            }))
            .on_action(cx.listener(|this, _: &TogglePretty, _, cx| {
                this.pretty_json = !this.pretty_json;
                cx.notify();
            }))
            .on_action(cx.listener(|this, _: &ToggleFollow, _, cx| {
                this.follow = !this.follow;
                cx.notify();
            }))
            .on_action(cx.listener(|this, _: &ToggleRegex, _, cx| {
                this.search_regex = !this.search_regex;
                let text = this.search_input.read(cx).value().to_string();
                this.set_query(text, cx);
            }))
            .on_action(cx.listener(|this, _: &ToggleCase, _, cx| {
                this.search_case_sensitive = !this.search_case_sensitive;
                let text = this.search_input.read(cx).value().to_string();
                this.set_query(text, cx);
            }))
            .on_action(cx.listener(|this, _: &FilterToMatches, _, cx| {
                this.filter_to_matches = !this.filter_to_matches;
                cx.notify();
            }))
            .on_action(cx.listener(|this, _: &NextMatch, _, cx| {
                this.cursor.go_next();
                cx.notify();
            }))
            .on_action(cx.listener(|this, _: &PrevMatch, _, cx| {
                this.cursor.go_prev();
                cx.notify();
            }))
            .on_action(cx.listener(|this, _: &CopyVisible, _, cx| this.copy_visible(cx)))
            .child(
                h_flex()
                    .h(u(kubyl_ui::sizes::TOOLBAR))
                    .px(u(8.0))
                    .gap(u(8.0))
                    .items_center()
                    .border_b_1()
                    .border_color(colors.border)
                    .bg(colors.panel)
                    .child(
                        Icon::new(if self.paused {
                            IconName::Play
                        } else {
                            IconName::Pause
                        })
                        .size(14.0)
                        .color(colors.text_dim),
                    )
                    .child(
                        div()
                            .cursor_pointer()
                            .on_mouse_down(
                                gpui::MouseButton::Left,
                                cx.listener(|this, _, _, cx| this.toggle_pause(cx)),
                            )
                            .child(if self.paused {
                                format!("Paused (+{})", self.paused_new_lines)
                            } else {
                                "Live".to_string()
                            }),
                    )
                    .child(div().flex_1().child(""))
                    .child(
                        div()
                            .w(u(220.0))
                            .child(Input::new(&self.search_input).appearance(false)),
                    )
                    .when(!match_label.is_empty(), |this| {
                        this.child(div().text_color(colors.text_dim).child(match_label))
                    })
                    .child(div().text_color(colors.text_faint).child(status)),
            )
            .child(
                h_flex()
                    .px(u(8.0))
                    .py(u(4.0))
                    .gap(u(6.0))
                    .border_b_1()
                    .border_color(colors.border_variant)
                    .children(LogLevel::ALL.iter().map(|level| {
                        let idx = level_index(*level);
                        let active = self.active_levels[idx];
                        let count = counts[idx];
                        let level = *level;
                        div()
                            .id(("level-chip", idx))
                            .px(u(6.0))
                            .rounded(u(4.0))
                            .cursor_pointer()
                            .when(active, |this| this.bg(colors.selection))
                            .when(!active, |this| this.opacity(0.5))
                            .on_mouse_down(
                                gpui::MouseButton::Left,
                                cx.listener(move |this, _, _, cx| this.toggle_level(level, cx)),
                            )
                            .child(format!("{} {}", level.label(), count))
                    })),
            )
            .child(
                uniform_list(
                    "log-lines",
                    row_count,
                    cx.processor(move |this: &mut Self, range: Range<usize>, _, cx| {
                        let colors = cx.colors().clone();
                        let rendered = this.rendered_lines();
                        range
                            .filter_map(|i| rendered.get(i).copied())
                            .map(|line| this.render_line(line, &colors).into_any_element())
                            .collect()
                    }),
                )
                .flex_1(),
            )
    }
}
