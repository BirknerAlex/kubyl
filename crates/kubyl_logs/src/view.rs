//! `ViewKind::Logs`: the log view (board 2 · Live logs).
//!
//! Streams through [`crate::stream`] into a ring buffer and renders the lines that pass the
//! level, field and search filters in a variable-height [`gpui::list`] (wrapped lines and pretty
//! JSON span several rows) that follows the tail. Two toolbars follow the mockup:
//!
//! 1. the source (workload, pod chips, container, since) with the Follow / Timestamps / Wrap /
//!    Previous / JSON toggles, pause and download;
//! 2. search (text or regex, case, filter to matches, "2 of 27"), level chips with counts, field
//!    filters added by clicking JSON keys, and the stream status.
//!
//! Filtering is incremental: `visible` (level + field filters) and `rendered` (plus "filter to
//! matches") are seq lists maintained per pushed/evicted line, and the list state is spliced to
//! match, so a 100k-line buffer never rescans on new data.

use std::collections::{BTreeSet, HashMap, HashSet, VecDeque};
use std::ops::Range;
use std::path::PathBuf;
use std::time::{Duration, Instant};

use futures::StreamExt as _;
use futures::channel::mpsc;
use gpui::{
    AnyElement, App, AppContext as _, ClipboardItem, Context, Entity, FocusHandle, Focusable,
    FollowMode, FontWeight, HighlightStyle, Hsla, InteractiveText, IntoElement, KeyBinding,
    ListAlignment, ListOffset, ListState, MouseButton, MouseDownEvent, SharedString, StyledText,
    Subscription, Task, Window, actions, div, list, prelude::*, px,
};
use gpui_component::button::{Button as MenuButton, ButtonVariants as _};
use gpui_component::input::{Input, InputEvent, InputState};
use gpui_component::menu::{DropdownMenu as _, PopupMenuItem};
use jiff::tz::TimeZone;
use k8s_openapi::api::apps::v1::{DaemonSet, Deployment, ReplicaSet, StatefulSet};
use k8s_openapi::api::batch::v1::Job;
use k8s_openapi::api::core::v1::{Pod, Service};
use k8s_openapi::apimachinery::pkg::apis::meta::v1::LabelSelector;
use kube::Api;
use kube::api::ListParams;
use kubyl_core::{
    ActionRegistry, ActionSpec, ClusterId, Notification, NotificationCenter, ResourceRef, TabView,
    Tone, ViewRequest,
};
use kubyl_kube::{ConnectionEvent, ConnectionManager};
use kubyl_ui::{ActiveColors, Chip, Colors, Icon, IconButton, IconName, fonts, h_flex, u, v_flex};

use crate::json::{self, FieldFilter, Token};
use crate::level::{LogLevel, detect_level, is_continuation};
use crate::line::{LogLine, color_index, pod_color, short_pod_name};
use crate::ring::LogRingBuffer;
use crate::search::{MatchCursor, Search};
use crate::sessions::{SessionId, SessionKind, SessionRegistry};
use crate::settings::LogsSettings;
use crate::stream::{
    self, ContainerFilter, LogOptions, LogSource, PodContainers, RawLine, Since, StreamEvent,
};

/// Key context of the whole view (toolbars, search input and lines).
pub(crate) const CONTEXT: &str = "LogsView";
/// Key context of the lines list; single-letter keys live here so typing in the search input
/// doesn't trigger them.
pub(crate) const LIST_CONTEXT: &str = "LogList";

actions!(
    logs,
    [
        /// Freezes the view; the buffer keeps filling and a counter shows new lines.
        TogglePause,
        /// Scrolls to the newest line and follows again.
        JumpToBottom,
        JumpToTop,
        ToggleWrap,
        ToggleTimestamps,
        /// Streams new lines (`kubectl logs -f`).
        ToggleFollow,
        /// Shows the previous instance of each container (`--previous`).
        TogglePrevious,
        /// Raw → inline `key=value` → pretty JSON.
        CycleJson,
        ToggleRegex,
        ToggleCase,
        /// Shows only the lines that match the search.
        FilterToMatches,
        NextMatch,
        PreviousMatch,
        FocusSearch,
        FocusLines,
        ClearSelection,
        /// Copies the selected lines (or the current match).
        CopySelection,
        /// Copies every line passing the filters.
        CopyVisible,
        /// Saves the lines passing the filters to a file.
        DownloadVisible,
        /// Saves the complete log of every streamed container to a file.
        DownloadFull,
        /// Removes the level, field and search filters.
        ClearFilters,
        /// Reconnects every stream from scratch.
        Restart,
    ]
);

/// Registers the log view's actions (palette, with keys in the view's contexts).
pub(crate) fn init(cx: &mut App) {
    let list = Some(LIST_CONTEXT);
    let view = Some(CONTEXT);
    let search = format!("{CONTEXT} > Input");
    cx.bind_keys([
        KeyBinding::new("space", TogglePause, list),
        KeyBinding::new("f", ToggleFollow, list),
        KeyBinding::new("w", ToggleWrap, list),
        KeyBinding::new("t", ToggleTimestamps, list),
        KeyBinding::new("p", TogglePrevious, list),
        KeyBinding::new("shift-j", CycleJson, list),
        KeyBinding::new("m", FilterToMatches, list),
        KeyBinding::new("n", NextMatch, list),
        KeyBinding::new("shift-n", PreviousMatch, list),
        KeyBinding::new("/", FocusSearch, list),
        KeyBinding::new("end", JumpToBottom, list),
        KeyBinding::new("shift-g", JumpToBottom, list),
        KeyBinding::new("home", JumpToTop, list),
        KeyBinding::new("g g", JumpToTop, list),
        KeyBinding::new("escape", ClearSelection, list),
        KeyBinding::new("secondary-c", CopySelection, list),
        KeyBinding::new("secondary-f", FocusSearch, view),
        KeyBinding::new("secondary-shift-c", CopyVisible, view),
        KeyBinding::new("secondary-s", DownloadVisible, view),
        KeyBinding::new("secondary-shift-s", DownloadFull, view),
        KeyBinding::new("alt-secondary-x", ToggleRegex, view),
        KeyBinding::new("alt-secondary-c", ToggleCase, view),
        KeyBinding::new("secondary-r", Restart, view),
        KeyBinding::new("escape", FocusLines, Some(&search)),
    ]);
    let specs = [
        ActionSpec::new("Logs: Pause or Resume", TogglePause),
        ActionSpec::new("Logs: Jump to Newest Line", JumpToBottom),
        ActionSpec::new("Logs: Jump to Oldest Line", JumpToTop),
        ActionSpec::new("Logs: Toggle Follow", ToggleFollow),
        ActionSpec::new("Logs: Toggle Timestamps", ToggleTimestamps),
        ActionSpec::new("Logs: Toggle Wrap", ToggleWrap),
        ActionSpec::new("Logs: Toggle Previous Container", TogglePrevious),
        ActionSpec::new("Logs: Cycle JSON (raw, inline, pretty)", CycleJson),
        ActionSpec::new("Logs: Toggle Regex Search", ToggleRegex),
        ActionSpec::new("Logs: Toggle Case-Sensitive Search", ToggleCase),
        ActionSpec::new("Logs: Filter to Matches", FilterToMatches),
        ActionSpec::new("Logs: Next Match", NextMatch),
        ActionSpec::new("Logs: Previous Match", PreviousMatch),
        ActionSpec::new("Logs: Search", FocusSearch),
        ActionSpec::new("Logs: Copy Selected Lines", CopySelection),
        ActionSpec::new("Logs: Copy Visible Lines", CopyVisible),
        ActionSpec::new("Logs: Download Visible Lines…", DownloadVisible),
        ActionSpec::new("Logs: Download Full Log…", DownloadFull),
        ActionSpec::new("Logs: Clear Filters", ClearFilters),
        ActionSpec::new("Logs: Restart Stream", Restart),
    ];
    for mut spec in specs {
        // Listed in the palette whenever a log view has focus; keys are bound above.
        spec.context = view.map(SharedString::new_static);
        ActionRegistry::register(cx, spec);
    }
}

/// How JSON lines are shown.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum JsonMode {
    Raw,
    /// `key=value` on one line, keys highlighted and clickable.
    Inline,
    /// Indented over several lines, keys highlighted and clickable.
    Pretty,
}

impl JsonMode {
    fn next(self) -> Self {
        match self {
            JsonMode::Raw => JsonMode::Inline,
            JsonMode::Inline => JsonMode::Pretty,
            JsonMode::Pretty => JsonMode::Raw,
        }
    }

    fn label(self) -> &'static str {
        match self {
            JsonMode::Raw => "Raw",
            JsonMode::Inline => "Inline",
            JsonMode::Pretty => "Pretty",
        }
    }
}

/// Where the stream starts: the last `n` lines, the last minutes, a point in time, or
/// everything.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SinceChoice {
    Tail(i64),
    Minutes(i64),
    Time(jiff::Timestamp),
    Everything,
}

impl SinceChoice {
    const MENU: [SinceChoice; 9] = [
        SinceChoice::Tail(100),
        SinceChoice::Tail(1000),
        SinceChoice::Tail(10_000),
        SinceChoice::Minutes(5),
        SinceChoice::Minutes(15),
        SinceChoice::Minutes(60),
        SinceChoice::Minutes(6 * 60),
        SinceChoice::Minutes(24 * 60),
        SinceChoice::Everything,
    ];

    /// The toolbar button: `last 1,000`, `since 15m`, `since 6h`, `everything`.
    pub fn label(self) -> String {
        match self {
            SinceChoice::Tail(n) => format!("last {}", thousands(n as u64)),
            SinceChoice::Minutes(m) if m >= 60 && m % 60 == 0 => format!("since {}h", m / 60),
            SinceChoice::Minutes(m) => format!("since {m}m"),
            SinceChoice::Time(t) => {
                let tz = TimeZone::system();
                let at = t.to_zoned(tz.clone());
                let today = jiff::Timestamp::now().to_zoned(tz).date();
                if at.date() == today {
                    format!("since {}", at.strftime("%H:%M"))
                } else {
                    format!("since {}", at.strftime("%m-%d %H:%M"))
                }
            }
            SinceChoice::Everything => "everything".into(),
        }
    }

    fn menu_label(self) -> String {
        match self {
            SinceChoice::Tail(n) => format!("Last {} lines", thousands(n as u64)),
            SinceChoice::Minutes(m) if m >= 60 && m % 60 == 0 => {
                format!("Last {} hour{}", m / 60, if m == 60 { "" } else { "s" })
            }
            SinceChoice::Minutes(m) => format!("Last {m} minutes"),
            SinceChoice::Time(_) => self.label(),
            SinceChoice::Everything => "Everything".into(),
        }
    }

    /// The stream options for this choice.
    pub fn options(self, follow: bool, previous: bool) -> LogOptions {
        let (since, tail_lines) = match self {
            SinceChoice::Tail(n) => (Since::Start, Some(n)),
            SinceChoice::Minutes(m) => (Since::Seconds(m * 60), None),
            SinceChoice::Time(t) => (Since::Time(t), None),
            SinceChoice::Everything => (Since::Start, None),
        };
        LogOptions {
            follow,
            since,
            tail_lines,
            previous,
        }
    }
}

/// Parses "since a time" input in the local time zone: `10:42`, `10:42:05`,
/// `2026-09-25 10:42` or an RFC 3339 timestamp.
pub fn parse_since_time(input: &str, tz: &TimeZone) -> Result<jiff::Timestamp, String> {
    let input = input.trim();
    if let Ok(timestamp) = input.parse::<jiff::Timestamp>() {
        return Ok(timestamp);
    }
    if let Ok(datetime) = input.parse::<jiff::civil::DateTime>() {
        return datetime
            .to_zoned(tz.clone())
            .map(|z| z.timestamp())
            .map_err(|e| e.to_string());
    }
    if let Ok(time) = input.parse::<jiff::civil::Time>() {
        let now = jiff::Timestamp::now().to_zoned(tz.clone());
        let mut at = now
            .date()
            .to_datetime(time)
            .to_zoned(tz.clone())
            .map_err(|e| e.to_string())?;
        // A time later than now means yesterday.
        if at.timestamp() > now.timestamp() {
            at = at.yesterday().map_err(|e| e.to_string())?;
        }
        return Ok(at.timestamp());
    }
    Err(format!(
        "{input:?} isn't a time (try 10:42 or 2026-09-25 10:42)"
    ))
}

/// `1284` → `1,284`.
pub fn thousands(n: u64) -> String {
    let digits = n.to_string();
    let mut out = String::with_capacity(digits.len() + digits.len() / 3);
    for (i, c) in digits.chars().enumerate() {
        if i > 0 && (digits.len() - i).is_multiple_of(3) {
            out.push(',');
        }
        out.push(c);
    }
    out
}

#[derive(Clone, Debug, PartialEq)]
enum Phase {
    NotConnected,
    Resolving,
    Streaming,
    /// Not following: everything was delivered.
    Done,
    /// Stopped from the Active Sessions panel.
    Stopped,
    Failed(String),
}

struct PodChip {
    name: SharedString,
    color_index: u8,
    gone: bool,
}

/// Lines per second over the last few seconds.
#[derive(Default)]
struct RateMeter {
    samples: VecDeque<(Instant, usize)>,
}

impl RateMeter {
    const WINDOW: Duration = Duration::from_secs(5);

    fn add(&mut self, now: Instant, lines: usize) {
        self.samples.push_back((now, lines));
        self.trim(now);
    }

    fn trim(&mut self, now: Instant) {
        while let Some((at, _)) = self.samples.front() {
            if now.duration_since(*at) > Self::WINDOW {
                self.samples.pop_front();
            } else {
                break;
            }
        }
    }

    fn per_second(&mut self, now: Instant) -> f64 {
        self.trim(now);
        let total: usize = self.samples.iter().map(|(_, n)| n).sum();
        total as f64 / Self::WINDOW.as_secs_f64()
    }
}

/// What the resolve step learned about the source.
struct Resolved {
    source: LogSource,
    /// Container names across the source's pods, for the container menu.
    containers: PodContainers,
}

pub struct LogsView {
    focus: FocusHandle,
    list_focus: FocusHandle,
    request: ViewRequest,
    target: ResourceRef,
    /// `deployment`, `pod`… (singular, lowercase).
    kind: String,
    // Source and options.
    source: Option<LogSource>,
    containers: PodContainers,
    container_filter: ContainerFilter,
    since: SinceChoice,
    follow: bool,
    previous: bool,
    show_timestamps: bool,
    wrap: bool,
    json_mode: JsonMode,
    time_zone: TimeZone,
    // Lines and filters.
    ring: LogRingBuffer,
    pods: Vec<PodChip>,
    active_levels: [bool; LogLevel::ALL.len()],
    field_filters: Vec<FieldFilter>,
    /// Per-level line counts over the whole buffer, kept incrementally.
    level_counts: [usize; LogLevel::ALL.len()],
    /// The last detected level per pod and container: stack-trace lines take it over.
    last_levels: HashMap<(SharedString, SharedString), LogLevel>,
    /// Seqs of lines passing the level and field filters, in ring order.
    visible: VecDeque<u64>,
    /// `visible` narrowed by "filter to matches": the rows of the list.
    rendered: VecDeque<u64>,
    /// Rows added to / evicted from `rendered` since the list state was last spliced.
    pending_appended: usize,
    pending_evicted: usize,
    // Search.
    search_input: Entity<InputState>,
    search: Option<Search>,
    search_error: Option<String>,
    search_regex: bool,
    search_case_sensitive: bool,
    filter_to_matches: bool,
    cursor: MatchCursor,
    // List.
    list_state: ListState,
    /// The rows while paused.
    frozen: Option<Vec<u64>>,
    /// The newest line when the view was paused: filter changes while paused show lines up
    /// to here.
    pause_limit: Option<u64>,
    paused_new_lines: u64,
    /// Selected lines: `(anchor, head)` seqs.
    selection: Option<(u64, u64)>,
    // Status.
    phase: Phase,
    last_error: Option<String>,
    reconnecting: HashSet<(SharedString, SharedString)>,
    rate: RateMeter,
    session_id: Option<SessionId>,
    session_status: Option<(SharedString, Tone)>,
    session_updated: Option<Instant>,
    /// Waits for the cluster to connect (a tab restored at startup).
    connect_subscription: Option<Subscription>,
    _stream_task: Option<Task<()>>,
    _subscriptions: Vec<Subscription>,
}

impl LogsView {
    pub fn new(target: Option<ResourceRef>, window: &mut Window, cx: &mut Context<Self>) -> Self {
        let has_target = target.is_some();
        let target = target.unwrap_or_else(|| {
            ResourceRef::object(
                ClusterId::new(""),
                kubyl_core::Gvr::new("", "v1", "pods"),
                None,
                String::new(),
            )
        });
        let kind = kind_label(&target.gvr.resource);
        let search_input = cx.new(|cx| InputState::new(window, cx).placeholder("Search logs…"));
        let settings = kubyl_settings::Settings::get::<LogsSettings>(cx).clone();
        let mut subscriptions = vec![cx.subscribe_in(
            &search_input,
            window,
            |this, input, event: &InputEvent, window, cx| match event {
                InputEvent::Change => {
                    let text = input.read(cx).value().to_string();
                    this.set_query(text, cx);
                }
                InputEvent::PressEnter { shift, .. } => {
                    if *shift {
                        this.go_to_match(false, window, cx);
                    } else {
                        this.go_to_match(true, window, cx);
                    }
                }
                _ => {}
            },
        )];
        subscriptions.push(cx.on_release(|this, cx| {
            // The tab was closed: drop the session row too (the stream task stops when `this`
            // is dropped, since only this view holds it).
            if let Some(id) = this.session_id.take() {
                SessionRegistry::remove(cx, id);
            }
        }));

        let list_state = ListState::new(0, ListAlignment::Top, px(1000.0));
        if settings.follow {
            list_state.set_follow_mode(FollowMode::Tail);
        }
        let mut this = Self {
            focus: cx.focus_handle(),
            list_focus: cx.focus_handle(),
            request: ViewRequest::for_resource(kubyl_core::ViewKind::Logs, target.clone()),
            target,
            kind,
            source: None,
            containers: PodContainers::default(),
            container_filter: ContainerFilter::default(),
            since: SinceChoice::Tail(settings.tail_lines.max(1)),
            follow: settings.follow,
            previous: false,
            show_timestamps: settings.timestamps,
            wrap: settings.wrap,
            json_mode: JsonMode::Raw,
            time_zone: TimeZone::system(),
            ring: LogRingBuffer::new(settings.ring_buffer_lines),
            pods: Vec::new(),
            active_levels: [true; LogLevel::ALL.len()],
            field_filters: Vec::new(),
            level_counts: [0; LogLevel::ALL.len()],
            last_levels: HashMap::new(),
            visible: VecDeque::new(),
            rendered: VecDeque::new(),
            pending_appended: 0,
            pending_evicted: 0,
            search_input,
            search: None,
            search_error: None,
            search_regex: false,
            search_case_sensitive: false,
            filter_to_matches: false,
            cursor: MatchCursor::default(),
            list_state,
            frozen: None,
            pause_limit: None,
            paused_new_lines: 0,
            selection: None,
            phase: Phase::Resolving,
            last_error: None,
            reconnecting: HashSet::new(),
            rate: RateMeter::default(),
            session_id: None,
            session_status: None,
            session_updated: None,
            connect_subscription: None,
            _stream_task: None,
            _subscriptions: subscriptions,
        };
        if has_target {
            this.start(cx);
        } else {
            this.phase = Phase::Failed("nothing to show".into());
        }
        this
    }

    fn is_selector_source(&self) -> bool {
        matches!(self.source, Some(LogSource::Selector { .. }))
    }

    // ----- Streaming -----

    fn options(&self) -> LogOptions {
        self.since.options(self.follow, self.previous)
    }

    /// Drops the lines and every derived structure (before a restart with new options).
    fn clear_lines(&mut self, cx: &mut Context<Self>) {
        let capacity = kubyl_settings::Settings::get::<LogsSettings>(cx).ring_buffer_lines;
        self.ring = LogRingBuffer::new(capacity);
        self.level_counts = [0; LogLevel::ALL.len()];
        self.last_levels.clear();
        self.visible.clear();
        self.rendered.clear();
        self.pending_appended = 0;
        self.pending_evicted = 0;
        self.cursor = MatchCursor::default();
        self.frozen = None;
        self.pause_limit = None;
        self.paused_new_lines = 0;
        self.selection = None;
        self.reconnecting.clear();
        self.last_error = None;
        self.rate = RateMeter::default();
        self.pods.clear();
        self.list_state.reset(0);
        if self.follow {
            self.list_state.set_follow_mode(FollowMode::Tail);
        }
    }

    /// Stops the current stream and starts again with the current options.
    fn restart(&mut self, cx: &mut Context<Self>) {
        self._stream_task = None;
        self.clear_lines(cx);
        self.start(cx);
        cx.notify();
    }

    fn start(&mut self, cx: &mut Context<Self>) {
        let Some(client) = ConnectionManager::global(cx)
            .read(cx)
            .client(&self.target.cluster)
        else {
            self.phase = Phase::NotConnected;
            self.wait_for_cluster(cx);
            return;
        };
        self.connect_subscription = None;
        self.phase = Phase::Resolving;
        let namespace = self.target.namespace.clone().unwrap_or_default();
        let resource = self.target.gvr.resource.clone();
        let name = self.target.name.clone().unwrap_or_default();
        let known = self.source.clone();
        let options = self.options();
        let filter = self.container_filter.clone();
        self.ensure_session(cx);

        let (tx, mut rx) = mpsc::unbounded();
        let resolve_client = client.clone();
        let resolve_namespace = namespace.clone();
        let resolve_task = known.is_none().then(|| {
            kubyl_core::spawn_kube(cx, async move {
                resolve(resolve_client, &resolve_namespace, &resource, &name).await
            })
        });
        self._stream_task = Some(cx.spawn(async move |this, cx| {
            let source = match resolve_task {
                None => known.expect("known when not resolving"),
                Some(task) => match task.await {
                    Ok(resolved) => {
                        let source = resolved.source.clone();
                        let alive = this
                            .update(cx, |this, cx| this.set_resolved(resolved, cx))
                            .is_ok();
                        if !alive {
                            return;
                        }
                        source
                    }
                    Err(err) => {
                        this.update(cx, |this, cx| {
                            this.phase = Phase::Failed(format!("{err:#}"));
                            this.update_session(true, cx);
                            cx.notify();
                        })
                        .ok();
                        return;
                    }
                },
            };
            // Kept alive for as long as this loop runs; dropping it aborts the stream.
            let _run = cx.update(|cx| {
                kubyl_core::spawn_kube(cx, async move {
                    stream::run(client, namespace, source, filter, options, tx).await
                })
            });
            if this
                .update(cx, |this, cx| {
                    this.phase = Phase::Streaming;
                    cx.notify();
                })
                .is_err()
            {
                return;
            }
            while let Some(first) = rx.next().await {
                let mut batch = vec![first];
                while let Ok(event) = rx.try_recv() {
                    batch.push(event);
                    if batch.len() > 2000 {
                        break;
                    }
                }
                if this
                    .update(cx, |this, cx| this.apply_events(batch, cx))
                    .is_err()
                {
                    return;
                }
                // Batches updates at ~60 Hz so high-volume streams don't re-render per line.
                cx.background_executor()
                    .timer(Duration::from_millis(16))
                    .await;
            }
        }));
    }

    /// Connects the cluster (a tab restored at startup opens before it) and starts streaming
    /// once it is up.
    fn wait_for_cluster(&mut self, cx: &mut Context<Self>) {
        if self.connect_subscription.is_some() {
            return;
        }
        let manager = ConnectionManager::global(cx);
        let cluster = self.target.cluster.clone();
        manager.update(cx, |manager, cx| manager.ensure_connected(&cluster, cx));
        self.connect_subscription = Some(cx.subscribe(
            &manager,
            move |this, manager, event: &ConnectionEvent, cx| {
                if let ConnectionEvent::StateChanged(id) = event
                    && *id == cluster
                    && manager.read(cx).state(id).is_connected()
                    && this.phase == Phase::NotConnected
                {
                    this.start(cx);
                    cx.notify();
                }
            },
        ));
    }

    fn set_resolved(&mut self, resolved: Resolved, cx: &mut Context<Self>) {
        self.source = Some(resolved.source);
        self.containers = resolved.containers;
        cx.notify();
    }

    fn set_selector(&mut self, selector: String, cx: &mut Context<Self>) {
        if selector.is_empty() || !self.is_selector_source() {
            return;
        }
        if self.source
            == Some(LogSource::Selector {
                label_selector: selector.clone(),
            })
        {
            return;
        }
        self.source = Some(LogSource::Selector {
            label_selector: selector,
        });
        self.restart(cx);
    }

    fn ensure_session(&mut self, cx: &mut Context<Self>) {
        if self.session_id.is_some() {
            return;
        }
        let weak = cx.weak_entity();
        let title = format!(
            "logs · {}/{}",
            self.kind,
            self.target.name.clone().unwrap_or_default()
        );
        let subtitle: SharedString = self.target.namespace.clone().unwrap_or_default().into();
        let id = SessionRegistry::add(
            cx,
            SessionKind::Logs,
            title,
            subtitle,
            "connecting…",
            Tone::Info,
            move |cx| {
                // Dropping the stream task drops the Tokio task it holds (the task-drop rule).
                weak.update(cx, |this, cx| {
                    this._stream_task = None;
                    this.session_id = None;
                    this.session_status = None;
                    this.phase = Phase::Stopped;
                    cx.notify();
                })
                .ok();
            },
        );
        self.session_id = Some(id);
    }

    /// Pushes the status to the Active Sessions row when it changed (at most once a second
    /// for rate-only changes, unless `force`).
    fn update_session(&mut self, force: bool, cx: &mut Context<Self>) {
        let Some(id) = self.session_id else { return };
        let now = Instant::now();
        let (label, tone) = self.status_line(now);
        let status = (label, tone);
        let tone_changed = self.session_status.as_ref().map(|s| s.1) != Some(tone);
        let due = self
            .session_updated
            .is_none_or(|at| now.duration_since(at) >= Duration::from_secs(1));
        if self.session_status.as_ref() != Some(&status) && (force || tone_changed || due) {
            SessionRegistry::set_status(cx, id, status.0.clone(), status.1);
            self.session_status = Some(status);
            self.session_updated = Some(now);
        }
    }

    /// `streaming · 3 pods · 1,284 lines · 42/s` and its tone.
    fn status_line(&mut self, now: Instant) -> (SharedString, Tone) {
        let pods = self.pods.iter().filter(|p| !p.gone).count();
        let pods = match pods {
            1 => "1 pod".to_string(),
            n => format!("{n} pods"),
        };
        let lines = format!("{} lines", thousands(self.ring.len() as u64));
        let (state, tone) = match &self.phase {
            Phase::NotConnected => ("not connected".to_string(), Tone::Muted),
            Phase::Resolving => ("connecting…".to_string(), Tone::Info),
            Phase::Stopped => ("stopped".to_string(), Tone::Muted),
            Phase::Failed(err) => (format!("error: {err}"), Tone::Bad),
            Phase::Done => ("complete".to_string(), Tone::Muted),
            Phase::Streaming if !self.reconnecting.is_empty() => (
                format!("{} reconnecting", self.reconnecting.len()),
                Tone::Warning,
            ),
            Phase::Streaming if !self.follow || self.previous => {
                ("loading".to_string(), Tone::Info)
            }
            Phase::Streaming => ("streaming".to_string(), Tone::Good),
        };
        let mut parts = vec![state, pods, lines];
        if matches!(self.phase, Phase::Streaming) && self.follow && !self.previous {
            parts.push(format!("{:.0}/s", self.rate.per_second(now)));
        }
        if self.ring.evicted() > 0 {
            parts.push(format!("{} dropped", thousands(self.ring.evicted())));
        }
        (parts.join(" · ").into(), tone)
    }

    fn apply_events(&mut self, events: Vec<StreamEvent>, cx: &mut Context<Self>) {
        let mut appended = 0usize;
        let mut force_session = false;
        for event in events {
            match event {
                StreamEvent::Lines(lines) => {
                    appended += lines.len();
                    for line in lines {
                        self.push_raw(line);
                    }
                }
                StreamEvent::PodJoined {
                    pod,
                    containers,
                    initial,
                } => {
                    let pod: SharedString = pod.into();
                    match self.pods.iter_mut().find(|p| p.name == pod) {
                        Some(chip) => chip.gone = false,
                        None => self.pods.push(PodChip {
                            color_index: color_index(&pod),
                            name: pod.clone(),
                            gone: false,
                        }),
                    }
                    for container in containers {
                        if !self.containers.all().contains(&container) {
                            self.containers.regular.push(container);
                        }
                    }
                    if !initial {
                        self.push_marker(pod.clone(), format!("── {pod} joined ──"));
                    }
                    force_session = true;
                }
                StreamEvent::PodGone(pod) => {
                    let pod: SharedString = pod.into();
                    if let Some(chip) = self.pods.iter_mut().find(|p| p.name == pod) {
                        chip.gone = true;
                    }
                    self.reconnecting.retain(|(p, _)| p != &pod);
                    self.push_marker(pod.clone(), format!("── {pod} deleted ──"));
                    force_session = true;
                }
                StreamEvent::Connected { pod, container } => {
                    self.reconnecting.remove(&(pod.into(), container.into()));
                    force_session = true;
                }
                StreamEvent::Reconnecting {
                    pod,
                    container,
                    error,
                } => {
                    let key: (SharedString, SharedString) = (pod.into(), container.into());
                    // One marker per outage, not one per retry.
                    if self.reconnecting.insert(key.clone()) {
                        let reason = error.map(|e| format!(": {e}")).unwrap_or_default();
                        self.push_marker(
                            key.0.clone(),
                            format!("── {}/{} reconnecting{reason} ──", key.0, key.1),
                        );
                    }
                    force_session = true;
                }
                StreamEvent::Ended {
                    pod,
                    container,
                    reason,
                } => {
                    let pod: SharedString = pod.into();
                    self.reconnecting
                        .remove(&(pod.clone(), container.clone().into()));
                    self.push_marker(pod.clone(), format!("── {pod}/{container} {reason} ──"));
                    force_session = true;
                }
                StreamEvent::Done => {
                    self.phase = Phase::Done;
                    force_session = true;
                }
                StreamEvent::Error(message) => {
                    self.last_error = Some(message);
                }
            }
        }
        if appended > 0 {
            self.rate.add(Instant::now(), appended);
        }
        if self.frozen.is_some() {
            self.paused_new_lines += appended as u64;
        }
        self.sync_list();
        self.update_session(force_session, cx);
        cx.notify();
    }

    fn push_raw(&mut self, raw: RawLine) {
        let pod: SharedString = raw.pod.into();
        let container: SharedString = raw.container.into();
        let level = match detect_level(&raw.text) {
            // A stack trace belongs to the line before it (ERROR filters keep the trace).
            LogLevel::Unknown if is_continuation(&raw.text) => self
                .last_levels
                .get(&(pod.clone(), container.clone()))
                .copied()
                .unwrap_or(LogLevel::Unknown),
            level => {
                self.last_levels
                    .insert((pod.clone(), container.clone()), level);
                level
            }
        };
        let search_match = self.search.as_ref().is_some_and(|s| s.matches(&raw.text));
        let passes_fields = self.field_filters.iter().all(|f| f.matches(&raw.text));
        let (timestamp, text) = (raw.timestamp, raw.text);
        let (seq, evicted) = self
            .ring
            .push_with(|seq| LogLine::with_level(seq, pod, container, timestamp, text, level));
        self.level_counts[level_index(level)] += 1;
        self.after_push(evicted);
        if self.active_levels[level_index(level)] && passes_fields {
            self.add_visible(seq, search_match);
        }
    }

    fn push_marker(&mut self, pod: SharedString, message: String) {
        let (seq, evicted) = self
            .ring
            .push_with(|seq| LogLine::marker(seq, pod, SharedString::default(), message));
        self.after_push(evicted);
        self.add_visible(seq, false);
    }

    /// Keeps the derived structures in sync with a line the ring evicted.
    fn after_push(&mut self, evicted: Option<LogLine>) {
        let Some(evicted) = evicted else { return };
        if !evicted.marker {
            self.level_counts[level_index(evicted.level)] -= 1;
        }
        if self.visible.front() == Some(&evicted.seq) {
            self.visible.pop_front();
        }
        if self.rendered.front() == Some(&evicted.seq) {
            self.rendered.pop_front();
            self.pending_evicted += 1;
        }
        self.cursor.remove_front_if(evicted.seq);
        if let Some((anchor, head)) = self.selection {
            let front = evicted.seq + 1;
            if anchor.max(head) < front {
                self.selection = None;
            } else {
                self.selection = Some((anchor.max(front), head.max(front)));
            }
        }
    }

    fn add_visible(&mut self, seq: u64, search_match: bool) {
        self.visible.push_back(seq);
        if search_match {
            self.cursor.push_back(seq);
        }
        let restrict = self.filter_to_matches && self.search.is_some();
        if !restrict || search_match {
            self.rendered.push_back(seq);
            self.pending_appended += 1;
        }
    }

    /// Splices the list state to match `rendered` after a batch (unless paused).
    fn sync_list(&mut self) {
        let (appended, evicted) = (self.pending_appended, self.pending_evicted);
        self.pending_appended = 0;
        self.pending_evicted = 0;
        if self.frozen.is_some() {
            return;
        }
        let old = self.list_state.item_count();
        // Rows can be added and evicted within one batch; only rows the list knew about are
        // removed from its front.
        let evict_old = evicted.min(old);
        let evict_new = evicted - evict_old;
        if evict_old > 0 {
            self.list_state.splice(0..evict_old, 0);
        }
        let add = appended.saturating_sub(evict_new);
        let count = self.list_state.item_count();
        if add > 0 {
            self.list_state.splice(count..count, add);
        }
        if self.list_state.item_count() != self.rendered.len() {
            // Shouldn't happen; recover rather than render wrong rows.
            self.list_state.reset(self.rendered.len());
        }
    }

    // ----- Filters -----

    fn set_query(&mut self, query: String, cx: &mut Context<Self>) {
        self.search_error = None;
        if query.is_empty() {
            self.search = None;
        } else {
            match Search::new(&query, self.search_regex, self.search_case_sensitive) {
                Ok(search) => self.search = Some(search),
                Err(err) => {
                    self.search = None;
                    self.search_error = Some(err);
                }
            }
        }
        self.rebuild_matches();
        if self.filter_to_matches {
            self.rebuild_rendered();
        }
        cx.notify();
    }

    fn refresh_query(&mut self, cx: &mut Context<Self>) {
        let text = self.search_input.read(cx).value().to_string();
        self.set_query(text, cx);
    }

    /// Recomputes the match set from `visible` (the query or a filter changed).
    fn rebuild_matches(&mut self) {
        let matches = match &self.search {
            Some(search) => self
                .visible
                .iter()
                .filter_map(|&seq| self.ring.get_by_seq(seq))
                .filter(|l| !l.marker && search.matches(&l.text))
                .map(|l| l.seq)
                .collect(),
            None => Vec::new(),
        };
        self.cursor.set_matches(matches);
    }

    /// Recomputes `rendered` from `visible` and resets the list, keeping the top row in view.
    fn rebuild_rendered(&mut self) {
        let top_seq = self.top_seq();
        self.rendered = if self.filter_to_matches
            && let Some(search) = &self.search
        {
            self.visible
                .iter()
                .copied()
                .filter(|&seq| {
                    self.ring
                        .get_by_seq(seq)
                        .is_some_and(|l| !l.marker && search.matches(&l.text))
                })
                .collect()
        } else {
            self.visible.clone()
        };
        self.pending_appended = 0;
        self.pending_evicted = 0;
        if self.frozen.is_some() {
            // Paused: filter the paused lines again, but keep newer ones out.
            let last = self.pause_limit;
            let rows: Vec<u64> = self
                .rendered
                .iter()
                .copied()
                .filter(|seq| last.is_some_and(|last| *seq <= last))
                .collect();
            self.list_state.reset(rows.len());
            self.frozen = Some(rows);
            return;
        }
        self.list_state.reset(self.rendered.len());
        if self.follow && !self.previous {
            self.list_state.set_follow_mode(FollowMode::Tail);
        } else if let Some(seq) = top_seq {
            let ix = match self.rendered.binary_search(&seq) {
                Ok(ix) | Err(ix) => ix,
            };
            self.list_state.scroll_to(ListOffset {
                item_ix: ix,
                offset_in_item: px(0.0),
            });
        }
    }

    /// Recomputes `visible` (levels or field filters changed), then matches and rows.
    fn rebuild_visible(&mut self) {
        self.visible = self
            .ring
            .iter()
            .filter(|line| {
                line.marker
                    || (self.active_levels[level_index(line.level)]
                        && self.field_filters.iter().all(|f| f.matches(&line.text)))
            })
            .map(|line| line.seq)
            .collect();
        self.rebuild_matches();
        self.rebuild_rendered();
    }

    fn top_seq(&self) -> Option<u64> {
        let ix = self.list_state.logical_scroll_top().item_ix;
        self.row_seq(ix)
    }

    fn toggle_level(&mut self, level: LogLevel, cx: &mut Context<Self>) {
        self.active_levels[level_index(level)] ^= true;
        self.rebuild_visible();
        cx.notify();
    }

    fn add_field_filter(&mut self, filter: FieldFilter, cx: &mut Context<Self>) {
        if !self.field_filters.contains(&filter) {
            self.field_filters.push(filter);
            self.rebuild_visible();
            cx.notify();
        }
    }

    fn remove_field_filter(&mut self, ix: usize, cx: &mut Context<Self>) {
        if ix < self.field_filters.len() {
            self.field_filters.remove(ix);
            self.rebuild_visible();
            cx.notify();
        }
    }

    fn clear_filters(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.active_levels = [true; LogLevel::ALL.len()];
        self.field_filters.clear();
        self.filter_to_matches = false;
        self.search_input
            .update(cx, |input, cx| input.set_value("", window, cx));
        self.search = None;
        self.rebuild_visible();
        cx.notify();
    }

    // ----- Navigation, pause, selection -----

    fn row_count(&self) -> usize {
        self.frozen
            .as_ref()
            .map_or(self.rendered.len(), |rows| rows.len())
    }

    fn row_seq(&self, ix: usize) -> Option<u64> {
        match &self.frozen {
            Some(rows) => rows.get(ix).copied(),
            None => self.rendered.get(ix).copied(),
        }
    }

    fn row_of_seq(&self, seq: u64) -> Option<usize> {
        match &self.frozen {
            Some(rows) => rows.binary_search(&seq).ok(),
            None => self.rendered.binary_search(&seq).ok(),
        }
    }

    fn go_to_match(&mut self, forward: bool, _: &mut Window, cx: &mut Context<Self>) {
        let seq = if forward {
            self.cursor.go_next()
        } else {
            self.cursor.go_prev()
        };
        if let Some(seq) = seq
            && let Some(ix) = self.row_of_seq(seq)
        {
            self.list_state.pause_following_tail();
            self.list_state.scroll_to_reveal_item(ix);
        }
        cx.notify();
    }

    fn toggle_pause(&mut self, cx: &mut Context<Self>) {
        if self.frozen.take().is_some() {
            self.pause_limit = None;
            self.paused_new_lines = 0;
            self.list_state.reset(self.rendered.len());
            if self.follow {
                self.list_state.set_follow_mode(FollowMode::Tail);
            }
        } else {
            self.frozen = Some(self.rendered.iter().copied().collect());
            self.pause_limit = self.ring.iter().last().map(|line| line.seq);
        }
        cx.notify();
    }

    fn jump_to_bottom(&mut self, cx: &mut Context<Self>) {
        if self.frozen.is_some() {
            self.toggle_pause(cx);
        }
        self.list_state.scroll_to_end();
        if self.follow {
            self.list_state.set_follow_mode(FollowMode::Tail);
        }
        cx.notify();
    }

    fn jump_to_top(&mut self, cx: &mut Context<Self>) {
        self.list_state.pause_following_tail();
        self.list_state.scroll_to(ListOffset {
            item_ix: 0,
            offset_in_item: px(0.0),
        });
        cx.notify();
    }

    fn select_row(&mut self, seq: u64, extend: bool, window: &mut Window, cx: &mut Context<Self>) {
        self.selection = match (extend, self.selection) {
            (true, Some((anchor, _))) => Some((anchor, seq)),
            _ => Some((seq, seq)),
        };
        self.list_focus.focus(window, cx);
        cx.notify();
    }

    fn is_selected(&self, seq: u64) -> bool {
        self.selection
            .is_some_and(|(a, b)| (a.min(b)..=a.max(b)).contains(&seq))
    }

    /// Whether lines come from several pods (show the pod column) or containers.
    fn multi_pod(&self) -> bool {
        self.pods.len() > 1 || self.is_selector_source()
    }

    fn multi_container(&self) -> bool {
        self.containers.select(&self.container_filter).len() > 1
    }

    /// A line as text for copy and download: `[timestamp] [pod/container] text`.
    fn line_text(&self, line: &LogLine) -> String {
        let mut out = String::new();
        if let Some(ts) = line.timestamp
            && self.show_timestamps
        {
            out.push_str(&ts.to_string());
            out.push(' ');
        }
        if !line.marker && (self.multi_pod() || self.multi_container()) {
            out.push_str(&line.pod);
            if !line.container.is_empty() {
                out.push('/');
                out.push_str(&line.container);
            }
            out.push(' ');
        }
        out.push_str(&line.text);
        out
    }

    fn copy_selection(&mut self, cx: &mut Context<Self>) {
        let seqs: Vec<u64> = match self.selection {
            Some((a, b)) => {
                let range = a.min(b)..=a.max(b);
                (0..self.row_count())
                    .filter_map(|ix| self.row_seq(ix))
                    .filter(|seq| range.contains(seq))
                    .collect()
            }
            None => self.cursor.current_line().into_iter().collect(),
        };
        if seqs.is_empty() {
            return;
        }
        let text = seqs
            .iter()
            .filter_map(|&seq| self.ring.get_by_seq(seq))
            .map(|line| self.line_text(line))
            .collect::<Vec<_>>()
            .join("\n");
        cx.write_to_clipboard(ClipboardItem::new_string(text));
    }

    fn visible_text(&self) -> String {
        (0..self.row_count())
            .filter_map(|ix| self.row_seq(ix))
            .filter_map(|seq| self.ring.get_by_seq(seq))
            .map(|line| self.line_text(line))
            .collect::<Vec<_>>()
            .join("\n")
    }

    fn file_name(&self) -> String {
        let name = self.target.name.clone().unwrap_or_else(|| "logs".into());
        format!("{name}.log")
    }

    /// Asks for a path and writes `contents` (produced on the background executor) there.
    fn save(&mut self, contents: Task<Result<String, String>>, cx: &mut Context<Self>) {
        let directory = dirs::download_dir()
            .or_else(dirs::home_dir)
            .unwrap_or_else(|| PathBuf::from("."));
        let path = cx.prompt_for_new_path(&directory, Some(&self.file_name()));
        cx.spawn(async move |_, cx| {
            let Ok(Ok(Some(path))) = path.await else {
                return;
            };
            let result = match contents.await {
                Ok(text) => {
                    let lines = text.lines().count();
                    let target = path.clone();
                    cx.background_executor()
                        .spawn(async move { std::fs::write(&target, text) })
                        .await
                        .map(|_| lines)
                        .map_err(|e| e.to_string())
                }
                Err(err) => Err(err),
            };
            cx.update(|cx| {
                let notification = match result {
                    Ok(lines) => Notification::success(format!(
                        "Saved {} lines to {}",
                        thousands(lines as u64),
                        path.display()
                    )),
                    Err(err) => Notification::error(format!("Couldn't save the log: {err}")),
                };
                NotificationCenter::push(cx, notification);
            });
        })
        .detach();
    }

    fn download_visible(&mut self, cx: &mut Context<Self>) {
        let text = self.visible_text();
        self.save(Task::ready(Ok(text)), cx);
    }

    fn download_full(&mut self, cx: &mut Context<Self>) {
        let Some(client) = ConnectionManager::global(cx)
            .read(cx)
            .client(&self.target.cluster)
        else {
            NotificationCenter::push(cx, Notification::error("The cluster isn't connected."));
            return;
        };
        let containers = self.containers.select(&self.container_filter);
        let targets: Vec<(String, String)> = self
            .pods
            .iter()
            .filter(|p| !p.gone)
            .flat_map(|p| {
                containers
                    .iter()
                    .map(move |c| (p.name.to_string(), c.clone()))
            })
            .collect();
        if targets.is_empty() {
            NotificationCenter::push(cx, Notification::warning("No streams to download."));
            return;
        }
        let namespace = self.target.namespace.clone().unwrap_or_default();
        let previous = self.previous;
        let prefix = targets.len() > 1;
        let task = kubyl_core::spawn_kube(cx, async move {
            let lines = stream::fetch_all(client, namespace, targets, previous)
                .await
                .map_err(|e| format!("{e:#}"))?;
            Ok(lines
                .into_iter()
                .map(|l| {
                    let ts = l.timestamp.map(|t| format!("{t} ")).unwrap_or_default();
                    if prefix {
                        format!("{ts}{}/{} {}", l.pod, l.container, l.text)
                    } else {
                        format!("{ts}{}", l.text)
                    }
                })
                .collect::<Vec<_>>()
                .join("\n"))
        });
        self.save(task, cx);
    }

    fn set_option(&mut self, change: impl FnOnce(&mut Self), cx: &mut Context<Self>) {
        let before = (self.options(), self.container_filter.clone());
        change(self);
        if (self.options(), self.container_filter.clone()) != before {
            self.restart(cx);
        } else {
            cx.notify();
        }
    }

    fn toggle_follow(&mut self, cx: &mut Context<Self>) {
        self.set_option(|this| this.follow = !this.follow, cx);
        if self.follow {
            self.list_state.set_follow_mode(FollowMode::Tail);
        } else {
            self.list_state.set_follow_mode(FollowMode::Normal);
        }
    }

    fn set_json_mode(&mut self, mode: JsonMode, cx: &mut Context<Self>) {
        self.json_mode = mode;
        self.list_state.remeasure();
        cx.notify();
    }

    fn toggle_wrap(&mut self, cx: &mut Context<Self>) {
        self.wrap = !self.wrap;
        self.list_state.remeasure();
        cx.notify();
    }

    fn toggle_timestamps(&mut self, cx: &mut Context<Self>) {
        self.show_timestamps = !self.show_timestamps;
        self.list_state.remeasure();
        cx.notify();
    }

    // ----- Rendering -----

    fn render_row(&mut self, ix: usize, window: &mut Window, cx: &mut Context<Self>) -> AnyElement {
        let colors = cx.colors().clone();
        let Some(seq) = self.row_seq(ix) else {
            return div().into_any_element();
        };
        let Some(line) = self.ring.get_by_seq(seq) else {
            // Evicted while paused.
            return div()
                .px(u(12.0))
                .h(u(20.0))
                .text_color(colors.text_faint)
                .child("… dropped from the buffer")
                .into_any_element();
        };
        let current_match = self.cursor.current_line() == Some(seq);
        let selected = self.is_selected(seq);
        let mono_size = u(12.0);
        let row = h_flex()
            .id(("log-line", seq as usize))
            .w_full()
            .items_start()
            .gap(u(12.0))
            .px(u(12.0))
            .font_family(fonts::MONO)
            .text_size(mono_size)
            .line_height(u(20.0))
            .on_mouse_down(
                MouseButton::Left,
                cx.listener(move |this, event: &MouseDownEvent, window, cx| {
                    this.select_row(seq, event.modifiers.shift, window, cx)
                }),
            );
        if line.marker {
            return row
                .text_color(colors.text_faint)
                .italic()
                .when(selected, |this| this.bg(colors.selection))
                .child(line.text.clone())
                .into_any_element();
        }
        let level_color = level_color(line.level, &colors);
        let is_error = matches!(line.level, LogLevel::Error | LogLevel::Fatal);
        let pod_label = self.pod_label(line, &colors);
        let message = self.render_message(line, current_match, &colors, window, cx);
        row.when(is_error && !selected, |this| {
            this.bg(colors.error_row_background)
        })
        .when(selected, |this| this.bg(colors.selection))
        .when(current_match, |this| {
            this.border_l_2().border_color(colors.yellow)
        })
        .when(self.show_timestamps, |this| {
            this.child(
                div()
                    .flex_none()
                    .text_color(colors.text_faint)
                    .child(self.format_time(line)),
            )
        })
        .children(pod_label)
        .child(
            div()
                .flex_none()
                .w(u(44.0))
                .text_color(level_color)
                .when(line.level != LogLevel::Unknown, |this| {
                    this.child(line.level.label())
                }),
        )
        .child(message)
        .into_any_element()
    }

    fn format_time(&self, line: &LogLine) -> String {
        match line.timestamp {
            Some(ts) => ts
                .to_zoned(self.time_zone.clone())
                .strftime("%H:%M:%S%.3f")
                .to_string(),
            None => " ".repeat(12),
        }
    }

    fn pod_label(&self, line: &LogLine, colors: &Colors) -> Option<AnyElement> {
        let multi_pod = self.multi_pod();
        let multi_container = self.multi_container();
        if !multi_pod && !multi_container {
            return None;
        }
        let gone = self.pods.iter().any(|p| p.gone && p.name == line.pod);
        let (label, color) = if multi_pod {
            let mut label = short_pod_name(&line.pod).to_string();
            if multi_container {
                label.push('/');
                label.push_str(&line.container);
            }
            (label, pod_color(line.pod_color_index, colors))
        } else {
            (
                line.container.to_string(),
                pod_color(color_index(&line.container), colors),
            )
        };
        Some(
            div()
                .flex_none()
                .max_w(u(220.0))
                .truncate()
                .text_color(color)
                .when(gone, |this| this.line_through().opacity(0.6))
                .child(label)
                .into_any_element(),
        )
    }

    fn render_message(
        &self,
        line: &LogLine,
        current_match: bool,
        colors: &Colors,
        _window: &mut Window,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let rendered = match self.json_mode {
            JsonMode::Raw => None,
            JsonMode::Inline => json::parse_object(&line.text).map(|v| json::inline(&v)),
            JsonMode::Pretty => json::parse_object(&line.text).map(|v| json::pretty(&v)),
        };
        let (text, mut highlights, fields) = match rendered {
            Some(rendered) => {
                let highlights: Vec<(Range<usize>, HighlightStyle)> = rendered
                    .tokens
                    .iter()
                    .map(|(range, token)| (range.clone(), token_style(*token, colors)))
                    .collect();
                (rendered.text, highlights, rendered.fields)
            }
            None => (line.text.clone(), Vec::new(), Vec::new()),
        };
        if let Some(search) = &self.search {
            let style = HighlightStyle {
                background_color: Some(if current_match {
                    colors.yellow.opacity(0.6)
                } else {
                    colors.yellow.opacity(0.3)
                }),
                color: Some(colors.text),
                ..Default::default()
            };
            let matches = search.ranges(&text).into_iter().map(|r| (r, style));
            highlights = gpui::combine_highlights(highlights, matches).collect();
        }
        let styled = StyledText::new(text).with_highlights(highlights);
        let base = div()
            .flex_1()
            .min_w_0()
            .text_color(colors.text)
            .map(|this| {
                if self.wrap || self.json_mode == JsonMode::Pretty {
                    this.whitespace_normal()
                } else {
                    this.whitespace_nowrap().overflow_hidden()
                }
            });
        if fields.is_empty() {
            return base.child(styled).into_any_element();
        }
        let ranges: Vec<Range<usize>> = fields.iter().map(|(r, _)| r.clone()).collect();
        let filters: Vec<FieldFilter> = fields.into_iter().map(|(_, f)| f).collect();
        let weak = cx.weak_entity();
        base.child(
            InteractiveText::new(("log-message", line.seq as usize), styled).on_click(
                ranges,
                move |ix, _, cx| {
                    if let Some(filter) = filters.get(ix).cloned() {
                        weak.update(cx, |this, cx| this.add_field_filter(filter, cx))
                            .ok();
                    }
                },
            ),
        )
        .into_any_element()
    }

    fn render_source_bar(&mut self, cx: &mut Context<Self>) -> AnyElement {
        let colors = cx.colors().clone();
        let weak = cx.weak_entity();
        let icon = match self.target.gvr.resource.as_str() {
            "pods" => IconName::Box,
            "services" => IconName::Network,
            _ => IconName::Layers,
        };
        let crumb = h_flex()
            .flex_none()
            .gap(u(6.0))
            .child(Icon::new(icon).size(14.0).color(colors.accent))
            .child(
                div()
                    .font_family(fonts::MONO)
                    .text_size(u(12.5))
                    .font_weight(FontWeight::MEDIUM)
                    .text_color(colors.text)
                    .child(format!(
                        "{}/{}",
                        self.kind,
                        self.target.name.clone().unwrap_or_default()
                    )),
            );
        let shown = 6;
        let pod_chips =
            h_flex()
                .flex_none()
                .gap(u(4.0))
                .children(self.pods.iter().take(shown).map(|pod| {
                    let color = pod_color(pod.color_index, &colors);
                    div()
                        .when(pod.gone, |this| this.opacity(0.5).line_through())
                        .child(
                            Chip::new(SharedString::from(short_pod_name(&pod.name).to_string()))
                                .mono()
                                .dot(color),
                        )
                }));
        let more = self.pods.len().saturating_sub(shown);

        // Container menu.
        let container_label: SharedString = match &self.container_filter.only {
            Some(only) => only.clone().into(),
            None if self.containers.regular.len() == 1 && !self.container_filter.init => {
                self.containers.regular[0].clone().into()
            }
            None => "all containers".into(),
        };
        let containers = self.containers.clone();
        let filter = self.container_filter.clone();
        let selector = match &self.source {
            Some(LogSource::Selector { label_selector }) => Some(label_selector.clone()),
            _ => None,
        };
        let menu_weak = weak.clone();
        let container_menu = menu_button("log-containers", IconName::Box, container_label, &colors)
            .dropdown_menu(move |menu, _, _| {
                let mut menu = menu.max_h(px(420.0)).scrollable(true);
                let item = |label: String, checked: bool, change: ContainerFilter| {
                    let weak = menu_weak.clone();
                    PopupMenuItem::new(label)
                        .checked(checked)
                        .on_click(move |_, _, cx| {
                            let change = change.clone();
                            weak.update(cx, |this, cx| {
                                this.set_option(|this| this.container_filter = change, cx)
                            })
                            .ok();
                        })
                };
                menu = menu.item(item(
                    "All containers".into(),
                    filter.only.is_none(),
                    ContainerFilter {
                        only: None,
                        ..filter.clone()
                    },
                ));
                menu = menu.separator();
                for name in &containers.regular {
                    menu = menu.item(item(
                        name.clone(),
                        filter.only.as_deref() == Some(name),
                        ContainerFilter {
                            only: Some(name.clone()),
                            ..filter.clone()
                        },
                    ));
                }
                if !containers.init.is_empty() {
                    menu = menu.separator().label("Init containers");
                    for name in &containers.init {
                        menu = menu.item(item(
                            name.clone(),
                            filter.only.as_deref() == Some(name),
                            ContainerFilter {
                                only: Some(name.clone()),
                                ..filter.clone()
                            },
                        ));
                    }
                }
                if !containers.ephemeral.is_empty() {
                    menu = menu.separator().label("Ephemeral containers");
                    for name in &containers.ephemeral {
                        menu = menu.item(item(
                            name.clone(),
                            filter.only.as_deref() == Some(name),
                            ContainerFilter {
                                only: Some(name.clone()),
                                ..filter.clone()
                            },
                        ));
                    }
                }
                if let Some(selector) = selector.clone() {
                    let weak = menu_weak.clone();
                    menu = menu.separator().item(
                        PopupMenuItem::new(format!("Label selector: {selector}…")).on_click(
                            move |_, window, cx| {
                                let weak = weak.clone();
                                kubyl_explorer::dialogs::prompt_text(
                                    "Stream the pods of a label selector".into(),
                                    "Label selector",
                                    selector.clone(),
                                    move |text, _, cx| {
                                        weak.update(cx, |this, cx| this.set_selector(text, cx))
                                            .ok();
                                    },
                                    window,
                                    cx,
                                );
                            },
                        ),
                    );
                }
                menu.separator()
                    .item(item(
                        "Include init containers".into(),
                        filter.init,
                        ContainerFilter {
                            init: !filter.init,
                            ..filter.clone()
                        },
                    ))
                    .item(item(
                        "Include ephemeral containers".into(),
                        filter.ephemeral,
                        ContainerFilter {
                            ephemeral: !filter.ephemeral,
                            ..filter.clone()
                        },
                    ))
            });

        // Since menu.
        let since = self.since;
        let since_weak = weak.clone();
        let since_menu = menu_button("log-since", IconName::Clock, since.label(), &colors)
            .dropdown_menu(move |menu, _, _| {
                let mut menu = menu;
                for (i, choice) in SinceChoice::MENU.into_iter().enumerate() {
                    if i == 3 || i == 8 {
                        menu = menu.separator();
                    }
                    let weak = since_weak.clone();
                    menu = menu.item(
                        PopupMenuItem::new(choice.menu_label())
                            .checked(choice == since)
                            .on_click(move |_, _, cx| {
                                weak.update(cx, |this, cx| {
                                    this.set_option(|this| this.since = choice, cx)
                                })
                                .ok();
                            }),
                    );
                }
                let weak = since_weak.clone();
                let checked = matches!(since, SinceChoice::Time(_));
                let label = if checked {
                    format!("{}…", since.label().replacen("since", "Since", 1))
                } else {
                    "Since a time…".to_string()
                };
                menu.item(PopupMenuItem::new(label).checked(checked).on_click(
                    move |_, window, cx| {
                        let weak = weak.clone();
                        let tz = TimeZone::system();
                        let hour_ago = jiff::Timestamp::now()
                            .checked_sub(jiff::SignedDuration::from_hours(1))
                            .unwrap_or_else(|_| jiff::Timestamp::now())
                            .to_zoned(tz.clone())
                            .strftime("%Y-%m-%d %H:%M")
                            .to_string();
                        kubyl_explorer::dialogs::prompt_text(
                            "Stream logs since".into(),
                            "Local time (10:42, 2026-09-25 10:42 or RFC 3339)",
                            hour_ago,
                            move |text, _, cx| match parse_since_time(&text, &tz) {
                                Ok(at) => {
                                    weak.update(cx, |this, cx| {
                                        this.set_option(
                                            |this| this.since = SinceChoice::Time(at),
                                            cx,
                                        )
                                    })
                                    .ok();
                                }
                                Err(err) => {
                                    NotificationCenter::push(cx, Notification::error(err));
                                }
                            },
                            window,
                            cx,
                        );
                    },
                ))
            });

        // JSON menu.
        let json_mode = self.json_mode;
        let json_weak = weak.clone();
        let json_menu = MenuButton::new("log-json")
            .ghost()
            .compact()
            .p_0()
            .child(toggle_chip("JSON", None, json_mode != JsonMode::Raw))
            .dropdown_menu(move |menu, _, _| {
                let mut menu = menu;
                for mode in [JsonMode::Raw, JsonMode::Inline, JsonMode::Pretty] {
                    let weak = json_weak.clone();
                    menu = menu.item(
                        PopupMenuItem::new(mode.label())
                            .checked(mode == json_mode)
                            .on_click(move |_, _, cx| {
                                weak.update(cx, |this, cx| this.set_json_mode(mode, cx))
                                    .ok();
                            }),
                    );
                }
                menu
            });

        let download_weak = weak.clone();
        let download = MenuButton::new("log-download")
            .ghost()
            .compact()
            .child(Icon::new(IconName::Download).size(14.0))
            .dropdown_menu(move |menu, _, _| {
                let visible = download_weak.clone();
                let full = download_weak.clone();
                menu.item(PopupMenuItem::new("Download visible lines…").on_click(
                    move |_, _, cx| {
                        visible
                            .update(cx, |this, cx| this.download_visible(cx))
                            .ok();
                    },
                ))
                .item(
                    PopupMenuItem::new("Download full log…").on_click(move |_, _, cx| {
                        full.update(cx, |this, cx| this.download_full(cx)).ok();
                    }),
                )
            });

        h_flex()
            .flex_none()
            .min_h(u(kubyl_ui::sizes::TOOLBAR))
            .py(u(6.0))
            .px(u(12.0))
            .gap(u(6.0))
            .items_center()
            .flex_wrap()
            .border_b_1()
            .border_color(colors.border)
            .bg(colors.panel)
            .child(crumb)
            .when(!self.pods.is_empty(), |this| {
                this.child(pod_chips).when(more > 0, |this| {
                    this.child(Chip::new(SharedString::from(format!("+{more}"))))
                })
            })
            .child(div().text_color(colors.text_faint).child("|"))
            .child(container_menu)
            .child(since_menu)
            .child(div().flex_1())
            .child(
                clickable(
                    "log-follow",
                    toggle_chip(
                        "Follow",
                        Some(IconName::Play),
                        self.follow && !self.previous,
                    ),
                )
                .on_click(cx.listener(|this, _, _, cx| this.toggle_follow(cx))),
            )
            .child(
                clickable(
                    "log-timestamps",
                    toggle_chip("Timestamps", None, self.show_timestamps),
                )
                .on_click(cx.listener(|this, _, _, cx| this.toggle_timestamps(cx))),
            )
            .child(
                clickable(
                    "log-wrap",
                    toggle_chip("Wrap", Some(IconName::WrapText), self.wrap),
                )
                .on_click(cx.listener(|this, _, _, cx| this.toggle_wrap(cx))),
            )
            .child(
                clickable("log-previous", toggle_chip("Previous", None, self.previous)).on_click(
                    cx.listener(|this, _, _, cx| {
                        this.set_option(|this| this.previous = !this.previous, cx)
                    }),
                ),
            )
            .child(json_menu)
            .child(
                IconButton::new(
                    "log-pause",
                    if self.frozen.is_some() {
                        IconName::Play
                    } else {
                        IconName::Pause
                    },
                )
                .icon_size(14.0)
                .toggled(self.frozen.is_some())
                .on_click(cx.listener(|this, _, _, cx| this.toggle_pause(cx))),
            )
            .child(download)
            .into_any_element()
    }

    fn render_search_bar(&mut self, cx: &mut Context<Self>) -> AnyElement {
        let colors = cx.colors().clone();
        let counts = self.level_counts;
        let match_label: Option<SharedString> = match (&self.search, &self.search_error) {
            (_, Some(_)) => Some("invalid".into()),
            (Some(_), None) => Some(match self.cursor.position() {
                Some(pos) => format!("{pos} of {}", thousands(self.cursor.count() as u64)).into(),
                None => "no matches".into(),
            }),
            (None, None) => None,
        };
        let search_box = h_flex()
            .flex_none()
            .w(u(320.0))
            .h(u(26.0))
            .px(u(8.0))
            .gap(u(7.0))
            .rounded(u(5.0))
            .border_1()
            .border_color(if self.search_error.is_some() {
                colors.red
            } else {
                colors.border
            })
            .bg(colors.input_background)
            .text_color(colors.text_dim)
            .child(Icon::new(IconName::Search).size(12.0))
            .child(
                div()
                    .flex_1()
                    .min_w_0()
                    .font_family(fonts::MONO)
                    .text_size(u(12.0))
                    .child(Input::new(&self.search_input).appearance(false)),
            )
            .when_some(match_label, |this, label| {
                this.child(
                    div()
                        .flex_none()
                        .text_size(u(11.5))
                        .when(self.search_error.is_some(), |this| {
                            this.text_color(colors.red)
                        })
                        .child(label),
                )
            })
            .child(
                kbd_toggle("log-regex", ".*", self.search_regex, &colors).on_click(cx.listener(
                    |this, _, _, cx| {
                        this.search_regex = !this.search_regex;
                        this.refresh_query(cx);
                    },
                )),
            )
            .child(
                kbd_toggle("log-case", "Aa", self.search_case_sensitive, &colors).on_click(
                    cx.listener(|this, _, _, cx| {
                        this.search_case_sensitive = !this.search_case_sensitive;
                        this.refresh_query(cx);
                    }),
                ),
            )
            .child(
                IconButton::new("log-filter-matches", IconName::Funnel)
                    .icon_size(12.0)
                    .toggled(self.filter_to_matches)
                    .on_click(cx.listener(|this, _, _, cx| {
                        this.filter_to_matches = !this.filter_to_matches;
                        this.rebuild_rendered();
                        cx.notify();
                    })),
            );

        let level_chips = h_flex().flex_none().gap(u(4.0)).children(
            LogLevel::ALL
                .iter()
                .copied()
                .filter(|level| {
                    counts[level_index(*level)] > 0
                        || matches!(
                            level,
                            LogLevel::Error | LogLevel::Warn | LogLevel::Info | LogLevel::Debug
                        )
                })
                .map(|level| {
                    let idx = level_index(level);
                    let count = counts[idx];
                    let label = if count > 0 {
                        format!("{} {}", level.label(), thousands(count as u64))
                    } else {
                        level.label().to_string()
                    };
                    let color = match level {
                        LogLevel::Error | LogLevel::Fatal => Some(colors.red),
                        LogLevel::Warn => Some(colors.yellow),
                        _ => None,
                    };
                    let active = self.active_levels[idx];
                    clickable(
                        ("log-level", idx),
                        div().when(!active, |this| this.opacity(0.55)).child(
                            Chip::new(SharedString::from(label))
                                .selected(active)
                                .when_some(color, |chip, color| chip.text_color(color)),
                        ),
                    )
                    .on_click(cx.listener(move |this, _, _, cx| this.toggle_level(level, cx)))
                }),
        );

        let field_chips =
            h_flex()
                .flex_none()
                .gap(u(4.0))
                .children(self.field_filters.iter().enumerate().map(|(ix, filter)| {
                    clickable(
                        ("log-field", ix),
                        Chip::new(SharedString::from(filter.label()))
                            .mono()
                            .selected(true)
                            .removable(),
                    )
                    .on_click(cx.listener(move |this, _, _, cx| this.remove_field_filter(ix, cx)))
                }));

        let (status, tone) = self.status_line(Instant::now());
        let status_color = kubyl_ui::tone_color(tone, &colors);
        h_flex()
            .flex_none()
            .min_h(u(38.0))
            .py(u(5.0))
            .px(u(12.0))
            .gap(u(8.0))
            .items_center()
            .flex_wrap()
            .border_b_1()
            .border_color(colors.border_variant)
            .bg(colors.subheader_background)
            .child(search_box)
            .child(level_chips)
            .child(field_chips)
            .child(div().flex_1())
            .when(self.frozen.is_some(), |this| {
                this.child(
                    div()
                        .flex_none()
                        .text_size(u(12.0))
                        .text_color(colors.yellow)
                        .child(format!(
                            "paused · +{} new",
                            thousands(self.paused_new_lines)
                        )),
                )
            })
            .when(
                matches!(self.phase, Phase::Stopped | Phase::Failed(_) | Phase::Done),
                |this| {
                    this.child(
                        clickable(
                            "log-restart",
                            Chip::new("Restart").icon(IconName::RefreshCw),
                        )
                        .on_click(cx.listener(|this, _, _, cx| this.restart(cx))),
                    )
                },
            )
            .child(
                h_flex()
                    .flex_none()
                    .gap(u(6.0))
                    .text_size(u(12.0))
                    .text_color(colors.text_dim)
                    .child(kubyl_ui::StatusDot::new(status_color))
                    .child(status),
            )
            .into_any_element()
    }
}

/// The toolbar dropdown button look (`.btn`, 24px): icon, label, chevron.
fn menu_button(
    id: &'static str,
    icon: IconName,
    label: impl Into<SharedString>,
    colors: &Colors,
) -> MenuButton {
    MenuButton::new(id).ghost().compact().p_0().child(
        h_flex()
            .h(u(24.0))
            .px(u(8.0))
            .gap(u(5.0))
            .rounded(u(5.0))
            .border_1()
            .border_color(colors.border)
            .bg(colors.button_background)
            .text_size(u(12.0))
            .text_color(colors.text)
            .child(Icon::new(icon).size(12.0))
            .child(label.into())
            .child(Icon::new(IconName::ChevronDown).size(11.0)),
    )
}

/// A chip that's on or off (`.chip.on`).
fn toggle_chip(label: &'static str, icon: Option<IconName>, on: bool) -> Chip {
    let chip = Chip::new(label).selected(on);
    match icon {
        Some(icon) => chip.icon(icon),
        None => chip,
    }
}

/// Wraps an element in a clickable, pointer-cursor div.
fn clickable(id: impl Into<gpui::ElementId>, child: impl IntoElement) -> gpui::Stateful<gpui::Div> {
    div().id(id).flex_none().cursor_pointer().child(child)
}

/// `.*` / `Aa` toggles inside the search box (`.kbd`).
fn kbd_toggle(
    id: &'static str,
    label: &'static str,
    on: bool,
    colors: &Colors,
) -> gpui::Stateful<gpui::Div> {
    div()
        .id(id)
        .flex_none()
        .cursor_pointer()
        .px(u(5.0))
        .rounded(u(4.0))
        .border_1()
        .font_family(fonts::MONO)
        .text_size(u(11.0))
        .line_height(u(15.0))
        .map(|this| {
            if on {
                this.border_color(colors.chip_selected_border)
                    .bg(colors.chip_selected_background)
                    .text_color(colors.chip_selected_text)
            } else {
                this.border_color(colors.border)
                    .bg(colors.kbd_background)
                    .text_color(colors.text_muted)
            }
        })
        .child(label)
}

fn token_style(token: Token, colors: &Colors) -> HighlightStyle {
    let color = match token {
        Token::Key => colors.accent,
        Token::String => colors.green,
        Token::Number => colors.orange,
        Token::Bool | Token::Null => colors.purple,
        Token::Punctuation => colors.text_dim,
    };
    HighlightStyle {
        color: Some(color),
        ..Default::default()
    }
}

fn level_color(level: LogLevel, colors: &Colors) -> Hsla {
    match level {
        LogLevel::Error | LogLevel::Fatal => colors.red,
        LogLevel::Warn => colors.yellow,
        LogLevel::Info => colors.green,
        LogLevel::Debug | LogLevel::Trace => colors.text_dim,
        LogLevel::Unknown => colors.text_faint,
    }
}

fn level_index(level: LogLevel) -> usize {
    LogLevel::ALL.iter().position(|l| *l == level).unwrap_or(0)
}

fn kind_label(resource: &str) -> String {
    match resource {
        "daemonsets" | "statefulsets" | "replicasets" | "deployments" | "jobs" | "pods" => {
            resource.trim_end_matches('s').to_string()
        }
        "services" => "service".into(),
        other => other.trim_end_matches('s').to_string(),
    }
}

/// Resolves a `ResourceRef` into a [`LogSource`]: pods stream themselves, workloads and
/// Services their pod selector. Also collects the container names for the container menu.
async fn resolve(
    client: kube::Client,
    namespace: &str,
    resource: &str,
    name: &str,
) -> anyhow::Result<Resolved> {
    let pods: Api<Pod> = Api::namespaced(client.clone(), namespace);
    if resource == "pods" {
        let pod = pods.get(name).await?;
        return Ok(Resolved {
            source: LogSource::Pod {
                pod: name.to_string(),
            },
            containers: PodContainers::of(&pod),
        });
    }
    let selector = match resource {
        "deployments" => selector_of(
            Api::<Deployment>::namespaced(client, namespace)
                .get(name)
                .await?
                .spec
                .map(|s| s.selector),
        ),
        "statefulsets" => selector_of(
            Api::<StatefulSet>::namespaced(client, namespace)
                .get(name)
                .await?
                .spec
                .map(|s| s.selector),
        ),
        "daemonsets" => selector_of(
            Api::<DaemonSet>::namespaced(client, namespace)
                .get(name)
                .await?
                .spec
                .map(|s| s.selector),
        ),
        "replicasets" => selector_of(
            Api::<ReplicaSet>::namespaced(client, namespace)
                .get(name)
                .await?
                .spec
                .map(|s| s.selector),
        ),
        "jobs" => selector_of(
            Api::<Job>::namespaced(client, namespace)
                .get(name)
                .await?
                .spec
                .and_then(|s| s.selector),
        ),
        "services" => {
            let service = Api::<Service>::namespaced(client, namespace)
                .get(name)
                .await?;
            let labels = service.spec.and_then(|s| s.selector).unwrap_or_default();
            labels
                .iter()
                .map(|(k, v)| format!("{k}={v}"))
                .collect::<Vec<_>>()
                .join(",")
        }
        other => anyhow::bail!("logs aren't supported for {other}"),
    };
    if selector.is_empty() {
        // An empty selector lists every pod in the namespace; refuse instead.
        anyhow::bail!("{resource}/{name} has no pod selector");
    }
    let mut containers = PodContainers::default();
    let listed = pods
        .list(&ListParams::default().labels(&selector).limit(20))
        .await?;
    let mut seen = BTreeSet::new();
    for pod in &listed.items {
        let found = PodContainers::of(pod);
        for (from, to) in [
            (found.regular, &mut containers.regular),
            (found.init, &mut containers.init),
            (found.ephemeral, &mut containers.ephemeral),
        ] {
            for name in from {
                if seen.insert(name.clone()) {
                    to.push(name);
                }
            }
        }
    }
    Ok(Resolved {
        source: LogSource::Selector {
            label_selector: selector,
        },
        containers,
    })
}

fn selector_of(selector: Option<LabelSelector>) -> String {
    selector.map(label_selector_to_string).unwrap_or_default()
}

/// Builds the string form of a `LabelSelector` that `kube`'s `ListParams::labels` expects,
/// combining `matchLabels` (`k=v`) with `matchExpressions` (`k in (a,b)`, `k notin (a,b)`, `k`
/// for `Exists`, `!k` for `DoesNotExist`), the same ANDing rules Kubernetes itself uses.
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
                // Unknown operator: skip it rather than build an invalid selector.
                _ => {}
            }
        }
    }
    parts.join(",")
}

impl Focusable for LogsView {
    fn focus_handle(&self, _: &App) -> FocusHandle {
        self.list_focus.clone()
    }
}

impl TabView for LogsView {
    fn tab_title(&self, _: &App) -> SharedString {
        format!("{} · logs", self.target.name.clone().unwrap_or_default()).into()
    }

    fn tab_icon(&self, _: &App) -> Option<SharedString> {
        Some(IconName::List.path())
    }

    fn view_request(&self, _: &App) -> Option<ViewRequest> {
        Some(self.request.clone())
    }
}

impl Render for LogsView {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let colors = cx.colors().clone();
        let source_bar = self.render_source_bar(cx);
        let search_bar = self.render_search_bar(cx);
        let following = self.list_state.is_following_tail();
        let empty = self.row_count() == 0;
        let placeholder: Option<SharedString> = empty.then(|| match &self.phase {
            Phase::NotConnected => "The cluster isn't connected.".into(),
            Phase::Resolving => "Connecting…".into(),
            Phase::Failed(err) => format!("Couldn't stream logs: {err}").into(),
            Phase::Stopped => "Stopped.".into(),
            _ if self.ring.is_empty() => "No log lines yet.".into(),
            _ => "No lines match the filters.".into(),
        });

        v_flex()
            .key_context(CONTEXT)
            .track_focus(&self.focus)
            .size_full()
            .bg(colors.background)
            .text_color(colors.text)
            .on_action(cx.listener(|this, _: &TogglePause, _, cx| this.toggle_pause(cx)))
            .on_action(cx.listener(|this, _: &JumpToBottom, _, cx| this.jump_to_bottom(cx)))
            .on_action(cx.listener(|this, _: &JumpToTop, _, cx| this.jump_to_top(cx)))
            .on_action(cx.listener(|this, _: &ToggleWrap, _, cx| this.toggle_wrap(cx)))
            .on_action(cx.listener(|this, _: &ToggleTimestamps, _, cx| this.toggle_timestamps(cx)))
            .on_action(cx.listener(|this, _: &ToggleFollow, _, cx| this.toggle_follow(cx)))
            .on_action(cx.listener(|this, _: &TogglePrevious, _, cx| {
                this.set_option(|this| this.previous = !this.previous, cx)
            }))
            .on_action(cx.listener(|this, _: &CycleJson, _, cx| {
                let next = this.json_mode.next();
                this.set_json_mode(next, cx)
            }))
            .on_action(cx.listener(|this, _: &ToggleRegex, _, cx| {
                this.search_regex = !this.search_regex;
                this.refresh_query(cx);
            }))
            .on_action(cx.listener(|this, _: &ToggleCase, _, cx| {
                this.search_case_sensitive = !this.search_case_sensitive;
                this.refresh_query(cx);
            }))
            .on_action(cx.listener(|this, _: &FilterToMatches, _, cx| {
                this.filter_to_matches = !this.filter_to_matches;
                this.rebuild_rendered();
                cx.notify();
            }))
            .on_action(
                cx.listener(|this, _: &NextMatch, window, cx| this.go_to_match(true, window, cx)),
            )
            .on_action(cx.listener(|this, _: &PreviousMatch, window, cx| {
                this.go_to_match(false, window, cx)
            }))
            .on_action(cx.listener(|this, _: &FocusSearch, window, cx| {
                this.search_input.update(cx, |input, cx| {
                    input.focus(window, cx);
                    input.select_all(window, cx);
                });
            }))
            .on_action(cx.listener(|this, _: &FocusLines, window, cx| {
                this.list_focus.focus(window, cx);
            }))
            .on_action(cx.listener(|this, _: &ClearSelection, _, cx| {
                this.selection = None;
                cx.notify();
            }))
            .on_action(cx.listener(|this, _: &CopySelection, _, cx| this.copy_selection(cx)))
            .on_action(cx.listener(|this, _: &CopyVisible, _, cx| {
                let text = this.visible_text();
                cx.write_to_clipboard(ClipboardItem::new_string(text));
            }))
            .on_action(cx.listener(|this, _: &DownloadVisible, _, cx| this.download_visible(cx)))
            .on_action(cx.listener(|this, _: &DownloadFull, _, cx| this.download_full(cx)))
            .on_action(
                cx.listener(|this, _: &ClearFilters, window, cx| this.clear_filters(window, cx)),
            )
            .on_action(cx.listener(|this, _: &Restart, _, cx| this.restart(cx)))
            .child(source_bar)
            .child(search_bar)
            .child(
                div()
                    .id("log-lines")
                    .key_context(LIST_CONTEXT)
                    .track_focus(&self.list_focus)
                    .relative()
                    .flex_1()
                    .min_h_0()
                    .py(u(6.0))
                    .when_some(placeholder, |this, message| {
                        this.child(
                            div()
                                .absolute()
                                .inset_0()
                                .flex()
                                .items_center()
                                .justify_center()
                                .text_color(colors.text_dim)
                                .child(message),
                        )
                    })
                    .child(
                        list(
                            self.list_state.clone(),
                            cx.processor(|this, ix: usize, window, cx| {
                                this.render_row(ix, window, cx)
                            }),
                        )
                        .size_full(),
                    )
                    .when(
                        self.follow && !following && !empty && self.frozen.is_none(),
                        |this| {
                            this.child(
                                div().absolute().bottom(u(12.0)).right(u(16.0)).child(
                                    clickable(
                                        "log-jump",
                                        Chip::new("Jump to newest").icon(IconName::ChevronDown),
                                    )
                                    .on_click(
                                        cx.listener(|this, _, _, cx| this.jump_to_bottom(cx)),
                                    ),
                                ),
                            )
                        },
                    ),
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

    #[test]
    fn since_choices_map_to_stream_options() {
        let options = SinceChoice::Tail(500).options(true, false);
        assert_eq!(options.tail_lines, Some(500));
        assert_eq!(options.since, Since::Start);
        let options = SinceChoice::Minutes(15).options(true, false);
        assert_eq!(options.since, Since::Seconds(900));
        assert_eq!(options.tail_lines, None);
        assert_eq!(SinceChoice::Minutes(15).label(), "since 15m");
        assert_eq!(SinceChoice::Minutes(360).label(), "since 6h");
        assert_eq!(SinceChoice::Tail(1000).label(), "last 1,000");
        assert_eq!(
            SinceChoice::Everything.options(false, true).tail_lines,
            None
        );
    }

    fn raw(pod: &str, text: &str) -> RawLine {
        RawLine {
            pod: pod.into(),
            container: "app".into(),
            timestamp: None,
            text: text.into(),
        }
    }

    fn open_view(
        cx: &mut gpui::TestAppContext,
    ) -> (tempfile::TempDir, gpui::WindowHandle<LogsView>) {
        let dir = tempfile::tempdir().unwrap();
        cx.update(|cx| {
            kubyl_core::init(cx);
            kubyl_settings::init_with_dir(cx, dir.path());
            kubyl_ui::init(cx);
            kubyl_settings::Settings::register::<LogsSettings>(cx);
            SessionRegistry::install(cx);
        });
        let window = cx.add_window(|window, cx| LogsView::new(None, window, cx));
        (dir, window)
    }

    #[gpui::test]
    fn filters_search_and_pause_stay_consistent(cx: &mut gpui::TestAppContext) {
        let (_dir, window) = open_view(cx);
        window
            .update(cx, |view, _, cx| {
                view.apply_events(
                    vec![
                        StreamEvent::PodJoined {
                            pod: "web-aaaaa".into(),
                            containers: vec!["app".into()],
                            initial: true,
                        },
                        StreamEvent::Lines(vec![
                            raw("web-aaaaa", "INFO started"),
                            raw("web-bbbbb", "ERROR upstream timeout"),
                            raw("web-aaaaa", "WARN slow"),
                        ]),
                    ],
                    cx,
                );
                assert_eq!(view.rendered.len(), 3);
                assert_eq!(view.list_state.item_count(), 3);
                assert_eq!(view.level_counts[level_index(LogLevel::Error)], 1);

                view.set_query("timeout".into(), cx);
                assert_eq!(view.cursor.count(), 1);
                view.filter_to_matches = true;
                view.rebuild_rendered();
                assert_eq!(view.rendered.len(), 1);
                assert_eq!(view.list_state.item_count(), 1);

                // New lines are filtered incrementally.
                view.apply_events(
                    vec![StreamEvent::Lines(vec![
                        raw("web-bbbbb", "INFO retry after TIMEOUT"),
                        raw("web-bbbbb", "INFO ok"),
                    ])],
                    cx,
                );
                assert_eq!(view.rendered.len(), 2);
                assert_eq!(view.list_state.item_count(), 2);
                assert_eq!(view.cursor.count(), 2);

                view.filter_to_matches = false;
                view.rebuild_rendered();
                assert_eq!(view.rendered.len(), 5);

                view.toggle_level(LogLevel::Info, cx);
                assert_eq!(view.rendered.len(), 2);
                view.toggle_level(LogLevel::Info, cx);

                // Pausing freezes the rows while the buffer keeps filling.
                view.toggle_pause(cx);
                view.apply_events(
                    vec![StreamEvent::Lines(vec![raw("web-aaaaa", "INFO x")])],
                    cx,
                );
                assert_eq!(view.row_count(), 5);
                assert_eq!(view.paused_new_lines, 1);
                // Filters still apply to the paused lines, without letting new ones in.
                view.toggle_level(LogLevel::Info, cx);
                assert_eq!(view.row_count(), 2);
                assert_eq!(view.list_state.item_count(), 2);
                view.toggle_level(LogLevel::Info, cx);
                assert_eq!(view.row_count(), 5);
                view.toggle_pause(cx);
                assert_eq!(view.row_count(), 6);
                assert_eq!(view.list_state.item_count(), 6);

                // A pod leaving marks its chip and adds a marker row.
                view.apply_events(vec![StreamEvent::PodGone("web-aaaaa".into())], cx);
                assert!(
                    view.pods
                        .iter()
                        .any(|p| p.name.as_ref() == "web-aaaaa" && p.gone)
                );
                assert_eq!(view.row_count(), 7);
            })
            .unwrap();
    }

    #[gpui::test]
    fn eviction_keeps_counts_and_rows_in_sync(cx: &mut gpui::TestAppContext) {
        let (_dir, window) = open_view(cx);
        window
            .update(cx, |view, _, cx| {
                view.ring = LogRingBuffer::new(5);
                // More lines than the capacity in one batch, then again in small batches.
                let lines: Vec<RawLine> = (0..12)
                    .map(|i| {
                        raw(
                            "p",
                            &format!("{} line {i}", if i % 2 == 0 { "ERROR" } else { "INFO" }),
                        )
                    })
                    .collect();
                view.apply_events(vec![StreamEvent::Lines(lines)], cx);
                assert_eq!(view.ring.len(), 5);
                assert_eq!(view.rendered.len(), 5);
                assert_eq!(view.list_state.item_count(), 5);
                for i in 0..7 {
                    view.apply_events(
                        vec![StreamEvent::Lines(vec![raw("p", &format!("WARN {i}"))])],
                        cx,
                    );
                    assert_eq!(view.list_state.item_count(), view.rendered.len());
                }
                let total: usize = view.level_counts.iter().sum();
                assert_eq!(total, 5);
                assert_eq!(view.level_counts[level_index(LogLevel::Warn)], 5);
                assert_eq!(view.ring.evicted(), 14);
            })
            .unwrap();
    }

    #[gpui::test]
    fn stack_traces_take_the_level_of_their_line(cx: &mut gpui::TestAppContext) {
        let (_dir, window) = open_view(cx);
        window
            .update(cx, |view, _, cx| {
                view.apply_events(
                    vec![StreamEvent::Lines(vec![
                        raw(
                            "kc-0",
                            "2026-09-25 10:25:16,400 ERROR [org.keycloak.Broker] (t-97) failed",
                        ),
                        raw("kc-0", "\tat org.keycloak.Broker.login(Broker.java:120)"),
                        raw(
                            "kc-1",
                            "2026-09-25 10:25:16,401 INFO  [org.keycloak] (t-1) other pod",
                        ),
                        raw("kc-0", "Caused by: java.io.IOException: closed"),
                        raw("kc-0", "\t... 12 more"),
                        raw(
                            "kc-0",
                            "2026-09-25 10:25:17,443 WARN  [org.keycloak.services] (t-97) next",
                        ),
                    ])],
                    cx,
                );
                assert_eq!(view.level_counts[level_index(LogLevel::Error)], 4);
                assert_eq!(view.level_counts[level_index(LogLevel::Info)], 1);
                assert_eq!(view.level_counts[level_index(LogLevel::Warn)], 1);
                assert_eq!(view.level_counts[level_index(LogLevel::Unknown)], 0);
            })
            .unwrap();
    }

    #[gpui::test]
    fn json_fields_filter_lines(cx: &mut gpui::TestAppContext) {
        let (_dir, window) = open_view(cx);
        window
            .update(cx, |view, _, cx| {
                view.apply_events(
                    vec![StreamEvent::Lines(vec![
                        raw("p", r#"{"level":"info","svc":"payments"}"#),
                        raw("p", r#"{"level":"error","svc":"auth"}"#),
                        raw("p", r#"{"level":"info","svc":"payments"}"#),
                    ])],
                    cx,
                );
                view.add_field_filter(
                    FieldFilter {
                        field: "svc".into(),
                        value: "payments".into(),
                    },
                    cx,
                );
                assert_eq!(view.rendered.len(), 2);
                // New lines respect the field filter too.
                view.apply_events(
                    vec![StreamEvent::Lines(vec![raw("p", r#"{"svc":"auth"}"#)])],
                    cx,
                );
                assert_eq!(view.rendered.len(), 2);
                view.remove_field_filter(0, cx);
                assert_eq!(view.rendered.len(), 4);
            })
            .unwrap();
    }

    /// 5,000 lines/s arrive as ~84 lines per 16 ms frame. Ingesting them (level detection,
    /// filters, search, list splicing) must leave most of the frame for rendering, which only
    /// touches the visible rows. Prints the cost; the bound is loose for debug builds.
    #[gpui::test]
    fn five_thousand_lines_per_second_fit_the_frame_budget(cx: &mut gpui::TestAppContext) {
        let (_dir, window) = open_view(cx);
        window
            .update(cx, |view, _, cx| {
                view.set_query("timeout".into(), cx);
                let start = Instant::now();
                let mut n = 0;
                for _frame in 0..60 {
                    let batch: Vec<RawLine> = (0..84)
                        .map(|_| {
                            n += 1;
                            let text = if n % 13 == 0 {
                                "ERROR upstream timeout after 2000ms".to_string()
                            } else {
                                format!(
                                    r#"{{"level":"info","msg":"POST /v1/checkout 200","order":"ord_{n}"}}"#
                                )
                            };
                            RawLine {
                                pod: format!("web-{}", n % 3),
                                container: "api".into(),
                                timestamp: Some(jiff::Timestamp::now()),
                                text,
                            }
                        })
                        .collect();
                    view.apply_events(vec![StreamEvent::Lines(batch)], cx);
                }
                let elapsed = start.elapsed();
                let per_frame = elapsed / 60;
                println!("5,040 lines in 60 batches: {elapsed:?} total, {per_frame:?} per frame");
                assert_eq!(view.ring.len(), 5040);
                assert_eq!(view.cursor.count(), 5040 / 13);
                assert!(per_frame < Duration::from_millis(16), "{per_frame:?} per frame");
            })
            .unwrap();
    }

    #[test]
    fn times_show_milliseconds() {
        let ts: jiff::Timestamp = "2026-09-25T10:42:17.902123456Z".parse().unwrap();
        assert_eq!(
            ts.to_zoned(TimeZone::UTC)
                .strftime("%H:%M:%S%.3f")
                .to_string(),
            "10:42:17.902"
        );
    }

    #[test]
    fn parses_since_times() {
        let tz = TimeZone::fixed(jiff::tz::offset(2));
        let at = parse_since_time("2026-09-25 10:42", &tz).unwrap();
        assert_eq!(at.to_string(), "2026-09-25T08:42:00Z");
        let at = parse_since_time("2026-09-25T10:42:05Z", &tz).unwrap();
        assert_eq!(at.to_string(), "2026-09-25T10:42:05Z");
        let at = parse_since_time("00:00", &tz).unwrap();
        assert!(at <= jiff::Timestamp::now());
        assert!(parse_since_time("yesterday-ish", &tz).is_err());
        let options = SinceChoice::Time(at).options(true, false);
        assert_eq!(options.since, Since::Time(at));
        assert_eq!(options.tail_lines, None);
    }

    #[test]
    fn thousands_groups_digits() {
        assert_eq!(thousands(0), "0");
        assert_eq!(thousands(999), "999");
        assert_eq!(thousands(1284), "1,284");
        assert_eq!(thousands(1_000_000), "1,000,000");
    }

    #[test]
    fn rate_meter_averages_over_its_window() {
        let mut meter = RateMeter::default();
        let start = Instant::now();
        meter.add(start, 100);
        meter.add(start + Duration::from_secs(1), 100);
        assert_eq!(meter.per_second(start + Duration::from_secs(2)), 40.0);
        assert_eq!(meter.per_second(start + Duration::from_secs(10)), 0.0);
    }
}
