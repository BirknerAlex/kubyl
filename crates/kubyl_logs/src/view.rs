//! `ViewKind::Logs`: the log view (board 2 · Live logs).
//!
//! Renders a virtualized list of ring-buffer lines with level chips, text/regex search and
//! pause/follow controls. Streaming itself lives in [`crate::stream`]; this module owns the
//! ring buffer, search/filter state and the kube object lookups needed to resolve a workload's
//! pod selector.

use std::collections::VecDeque;
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
use k8s_openapi::apimachinery::pkg::apis::meta::v1::LabelSelector;
use kube::Api;
use kubyl_core::{ClusterId, ResourceRef, TabView, Tone, ViewRequest};
use kubyl_kube::ConnectionManager;
use kubyl_ui::{ActiveColors, Colors, Icon, IconName, fonts, h_flex, u, v_flex};

use crate::json;
use crate::level::{LogLevel, detect_level};
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
    /// The status last pushed to `SessionRegistry`, so `apply_events` only calls
    /// `set_status` when it actually changes instead of ~60 times a second.
    last_session_status: Option<(SharedString, Tone)>,
    /// Cached per-level line counts, updated incrementally as lines are pushed/evicted instead
    /// of rescanning the whole (up to 100k-line) ring every render.
    level_counts: [usize; LogLevel::ALL.len()],
    /// Seqs of lines passing the level filter, in ring order. Updated incrementally as lines are
    /// pushed/evicted (`push_line_indexed`); only rebuilt from scratch when `active_levels`
    /// changes (`rebuild_visible_cache`).
    visible_cache: VecDeque<u64>,
    /// `visible_cache` further narrowed by `filter_to_matches`, maintained the same way.
    rendered_cache: VecDeque<u64>,
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
            last_session_status: None,
            level_counts: [0; LogLevel::ALL.len()],
            visible_cache: VecDeque::new(),
            rendered_cache: VecDeque::new(),
            _stream_task: None,
            _subscriptions: vec![subscription],
        };
        let release_subscription = cx.on_release(|this, cx| {
            // The tab was closed without the user clicking "stop" in the Active Sessions panel:
            // drop the session row too (the stream task itself stops when `this` is dropped,
            // since `_stream_task`/`_run_task` are held only by this view).
            if let Some(id) = this.session_id.take() {
                SessionRegistry::remove(cx, id);
            }
        });
        this._subscriptions.push(release_subscription);
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
            let stop_weak = this.clone();
            this.update(cx, |this, cx| {
                let id = SessionRegistry::add(
                    cx,
                    SessionKind::Logs,
                    title,
                    subtitle,
                    "streaming",
                    Tone::Good,
                    move |cx| {
                        // Dropping `_stream_task` drops the outer spawned future, which in turn
                        // drops the `_run_task` local it holds, aborting the kube stream (the
                        // Task-drop rule from AGENTS.md).
                        stop_weak
                            .update(cx, |this, cx| {
                                this._stream_task = None;
                                this.session_id = None;
                                this.status = "stopped".into();
                                cx.notify();
                            })
                            .ok();
                    },
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
        // The most specific status this batch produced, if any (e.g. `Reconnecting`). Only
        // falls back to "streaming" below when nothing more specific happened this batch, so a
        // `Reconnecting` event isn't immediately stomped by the default at the end.
        let mut pending_status: Option<(SharedString, Tone)> = None;
        for event in events {
            match event {
                StreamEvent::Line {
                    pod,
                    container,
                    text,
                    ..
                } => {
                    self.push_line_indexed(pod, container, text, false);
                    appended += 1;
                }
                StreamEvent::PodJoined(pod) => {
                    let message = format!("── {pod} joined ──");
                    self.push_line_indexed(pod, String::new(), message, true);
                }
                StreamEvent::PodGone(pod) => {
                    let message = format!("── {pod} left ──");
                    self.push_line_indexed(pod, String::new(), message, true);
                }
                StreamEvent::Reconnecting { pod, container } => {
                    self.push_line_indexed(pod, container, "── reconnecting… ──".to_string(), true);
                    pending_status = Some(("reconnecting".into(), Tone::Warning));
                }
                StreamEvent::Error(message) => {
                    self.status = format!("error: {message}").into();
                }
            }
        }
        if self.paused {
            self.paused_new_lines += appended;
        } else if let Some(id) = self.session_id {
            let status = pending_status.unwrap_or(("streaming".into(), Tone::Good));
            // Avoid calling into the registry (which notifies its own subscribers) ~60 times a
            // second when nothing actually changed.
            if self.last_session_status.as_ref() != Some(&status) {
                SessionRegistry::set_status(cx, id, status.0.clone(), status.1);
                self.last_session_status = Some(status);
            }
        }
        cx.notify();
    }

    /// Pushes one line (or gap marker) into the ring and keeps `level_counts`, `visible_cache`,
    /// `rendered_cache` and the search `cursor` in sync incrementally — an O(1) amortized update
    /// per line instead of rescanning the whole (up to 100k-line) ring on every batch.
    fn push_line_indexed(
        &mut self,
        pod: String,
        container: String,
        text: String,
        gap: bool,
    ) -> u64 {
        let level = if gap {
            LogLevel::Unknown
        } else {
            detect_level(&text)
        };
        let search_matches = self.search.as_ref().is_some_and(|s| s.matches(&text));

        let (seq, evicted) = if gap {
            self.ring.push_gap(pod, container, text)
        } else {
            self.ring.push(pod, container, text)
        };

        if !gap {
            self.level_counts[level_index(level)] += 1;
        }
        if let Some(evicted_line) = &evicted {
            if !evicted_line.gap_marker {
                self.level_counts[level_index(evicted_line.level)] -= 1;
            }
            if self.visible_cache.front() == Some(&evicted_line.seq) {
                self.visible_cache.pop_front();
            }
            if self.rendered_cache.front() == Some(&evicted_line.seq) {
                self.rendered_cache.pop_front();
            }
            self.cursor.remove_front_if(evicted_line.seq);
        }

        let passes_level = gap || self.active_levels[level_index(level)];
        if passes_level {
            self.visible_cache.push_back(seq);
            if search_matches {
                self.cursor.push_back(seq);
            }
            let restrict_to_matches = self.filter_to_matches && self.search.is_some();
            if !restrict_to_matches || search_matches {
                self.rendered_cache.push_back(seq);
            }
        }
        seq
    }

    fn set_query(&mut self, query: String, cx: &mut Context<Self>) {
        if query.is_empty() {
            self.search = None;
        } else {
            match Search::new(&query, self.search_regex, self.search_case_sensitive) {
                Ok(search) => self.search = Some(search),
                Err(err) => self.status = format!("bad pattern: {err}").into(),
            }
        }
        // The query changed: this is exactly the case where resetting match navigation back to
        // the first match is correct (unlike new data arriving under an unchanged query, which
        // is handled incrementally by `push_line_indexed` and must not reset it).
        self.rebuild_matches_and_rendered();
        cx.notify();
    }

    /// Full recompute of the search match set (and, in turn, `rendered_cache`) from
    /// `visible_cache`. Only call this when the search query/regex/case-sensitivity or the level
    /// filter changes — everything else is maintained incrementally.
    fn rebuild_matches_and_rendered(&mut self) {
        let matches = match &self.search {
            Some(search) => {
                let visible = self.filtered_lines();
                search.find_all(&visible)
            }
            None => Vec::new(),
        };
        self.cursor.set_matches(matches);
        self.rebuild_rendered_cache();
    }

    /// Rebuilds `rendered_cache` from `visible_cache` and the current `filter_to_matches`/search
    /// state, without touching match navigation.
    fn rebuild_rendered_cache(&mut self) {
        self.rendered_cache = if self.filter_to_matches
            && let Some(search) = &self.search
        {
            self.visible_cache
                .iter()
                .copied()
                .filter(|&seq| {
                    self.ring
                        .get_by_seq(seq)
                        .is_some_and(|l| search.matches(&l.text))
                })
                .collect()
        } else {
            self.visible_cache.clone()
        };
    }

    /// Full rebuild of `visible_cache` from the ring — only needed when `active_levels` changes
    /// (new/evicted lines are handled incrementally by `push_line_indexed`).
    fn rebuild_visible_cache(&mut self) {
        self.visible_cache = self
            .ring
            .iter()
            .filter(|line| line.gap_marker || self.active_levels[level_index(line.level)])
            .map(|line| line.seq)
            .collect();
        self.rebuild_matches_and_rendered();
    }

    /// Lines passing the level filter, in ring-buffer order.
    fn filtered_lines(&self) -> Vec<&LogLine> {
        self.visible_cache
            .iter()
            .filter_map(|&seq| self.ring.get_by_seq(seq))
            .collect()
    }

    fn rendered_lines(&self) -> Vec<&LogLine> {
        self.rendered_cache
            .iter()
            .filter_map(|&seq| self.ring.get_by_seq(seq))
            .collect()
    }

    fn level_counts(&self) -> [usize; LogLevel::ALL.len()] {
        self.level_counts
    }

    fn toggle_level(&mut self, level: LogLevel, cx: &mut Context<Self>) {
        self.active_levels[level_index(level)] ^= true;
        self.rebuild_visible_cache();
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
        // `uniform_list` virtualizes rows at a fixed height, so multi-line text would overlap
        // adjacent rows. `json::pretty` is multi-line by design, so use the flat `key=value`
        // rendering here instead — still easier to scan than raw JSON, without breaking
        // virtualization. (A real fix needs variable-height rows, e.g. `gpui::list`/`ListState`;
        // out of scope for this pass.)
        let display_text = if self.pretty_json
            && let Some(value) = json::parse_object(&line.text)
        {
            json::inline(&value)
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
            let selector = obj.spec.map(|s| s.selector).unwrap_or_default();
            Ok(LogSource::Workload {
                label_selector: label_selector_to_string(selector),
            })
        }
        "statefulsets" => {
            let api: Api<StatefulSet> = Api::namespaced(client, namespace);
            let obj = api.get(name).await?;
            let selector = obj.spec.map(|s| s.selector).unwrap_or_default();
            Ok(LogSource::Workload {
                label_selector: label_selector_to_string(selector),
            })
        }
        "daemonsets" => {
            let api: Api<DaemonSet> = Api::namespaced(client, namespace);
            let obj = api.get(name).await?;
            let selector = obj.spec.map(|s| s.selector).unwrap_or_default();
            Ok(LogSource::Workload {
                label_selector: label_selector_to_string(selector),
            })
        }
        "jobs" => {
            let api: Api<Job> = Api::namespaced(client, namespace);
            let obj = api.get(name).await?;
            let selector = obj.spec.and_then(|s| s.selector).unwrap_or_default();
            Ok(LogSource::Workload {
                label_selector: label_selector_to_string(selector),
            })
        }
        other => anyhow::bail!("logs aren't supported for {other}"),
    }
}

/// Builds the string form of a `LabelSelector` that `kube`'s `ListParams::labels` expects,
/// combining `matchLabels` (`k=v`) with `matchExpressions` (`k in (a,b)`, `k notin (a,b)`, `k`
/// for `Exists`, `!k` for `DoesNotExist`) — the same selector-string ANDing rules Kubernetes
/// itself uses. Workloads that only use `matchExpressions` previously got an empty selector here,
/// which lists (and streams logs for) every pod in the namespace instead of the workload's own.
fn label_selector_to_string(selector: LabelSelector) -> String {
    let mut parts = Vec::new();
    if let Some(labels) = selector.match_labels {
        for (key, value) in labels {
            parts.push(format!("{key}={value}"));
        }
    }
    if let Some(expressions) = selector.match_expressions {
        for expr in expressions {
            let key = expr.key;
            match expr.operator.as_str() {
                "In" => {
                    let values = expr.values.unwrap_or_default().join(",");
                    parts.push(format!("{key} in ({values})"));
                }
                "NotIn" => {
                    let values = expr.values.unwrap_or_default().join(",");
                    parts.push(format!("{key} notin ({values})"));
                }
                "Exists" => parts.push(key),
                "DoesNotExist" => parts.push(format!("!{key}")),
                _ => {
                    // Unknown/future operator: skip it rather than build an invalid selector
                    // string that would make the whole `list` call fail.
                }
            }
        }
    }
    parts.join(",")
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
                // Doesn't change the match set itself, just what's rendered, so match
                // navigation (`cursor`) is left untouched.
                this.rebuild_rendered_cache();
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn label_selector_combines_match_labels_and_match_expressions() {
        let selector = LabelSelector {
            match_labels: Some(std::collections::BTreeMap::from([(
                "app".to_string(),
                "web".to_string(),
            )])),
            match_expressions: Some(vec![
                k8s_openapi::apimachinery::pkg::apis::meta::v1::LabelSelectorRequirement {
                    key: "tier".to_string(),
                    operator: "In".to_string(),
                    values: Some(vec!["frontend".to_string(), "edge".to_string()]),
                },
                k8s_openapi::apimachinery::pkg::apis::meta::v1::LabelSelectorRequirement {
                    key: "env".to_string(),
                    operator: "NotIn".to_string(),
                    values: Some(vec!["dev".to_string()]),
                },
                k8s_openapi::apimachinery::pkg::apis::meta::v1::LabelSelectorRequirement {
                    key: "canary".to_string(),
                    operator: "DoesNotExist".to_string(),
                    values: None,
                },
                k8s_openapi::apimachinery::pkg::apis::meta::v1::LabelSelectorRequirement {
                    key: "region".to_string(),
                    operator: "Exists".to_string(),
                    values: None,
                },
            ]),
        };
        assert_eq!(
            label_selector_to_string(selector),
            "app=web,tier in (frontend,edge),env notin (dev),!canary,region"
        );
    }

    #[test]
    fn label_selector_with_only_match_expressions_is_not_empty() {
        // A workload using only `matchExpressions` (no `matchLabels`) must not resolve to an
        // empty selector string, which would list every pod in the namespace instead of just
        // this workload's.
        let selector = LabelSelector {
            match_labels: None,
            match_expressions: Some(vec![
                k8s_openapi::apimachinery::pkg::apis::meta::v1::LabelSelectorRequirement {
                    key: "app".to_string(),
                    operator: "In".to_string(),
                    values: Some(vec!["payments".to_string()]),
                },
            ]),
        };
        assert_eq!(label_selector_to_string(selector), "app in (payments)");
    }

    #[test]
    fn empty_label_selector_yields_empty_string() {
        assert_eq!(label_selector_to_string(LabelSelector::default()), "");
    }
}
