//! `ViewKind::Terminal` and the terminals of the bottom-dock panel (board 2 · exec shell).
//!
//! [`TerminalView`] runs one session (exec, attach, ephemeral debug container or node shell)
//! and paints [`crate::grid::TerminalGrid`] on a real cell grid: the cell size comes from GPUI's
//! text system (the advance of `m` in the terminal font), glyph runs are shaped with that width
//! forced so columns line up, and backgrounds, cursor, selection and link underlines are
//! painted per cell. Mouse selection, scrollback, links (Cmd/Ctrl-click), mouse reporting,
//! bracketed paste and xterm key encoding are handled here; the websocket runs off the UI
//! thread in [`crate::exec`].

use std::ops::Range;
use std::time::Duration;

use futures::StreamExt as _;
use futures::channel::mpsc;
use gpui::{
    Action, App, BorderStyle, Bounds, ClipboardItem, Context, FocusHandle, Focusable, Font,
    FontStyle, FontWeight, Hsla, IntoElement, KeyBinding, KeyDownEvent, Modifiers, MouseButton,
    MouseDownEvent, MouseMoveEvent, MouseUpEvent, Pixels, Point, ScrollWheelEvent, SharedString,
    StrikethroughStyle, Subscription, Task, TextAlign, TextRun, UnderlineStyle, Window, actions,
    canvas, div, fill, outline, point, prelude::*, px, size,
};
use gpui_component::button::{Button as MenuButton, ButtonVariants as _};
use gpui_component::menu::{DropdownMenu as _, PopupMenuItem};
use kubyl_core::{
    ClusterId, Notification, NotificationCenter, ResourceRef, TabView, Tone, ViewRequest,
};
use kubyl_kube::{ConnectionEvent, ConnectionManager};
use kubyl_logs::sessions::{SessionId, SessionKind, SessionRegistry};
use kubyl_ui::{ActiveColors, Colors, Icon, IconButton, IconName, fonts, h_flex, u};
use schemars::JsonSchema;
use serde::Deserialize;

use crate::exec::{self, DebugSpec, ExecTarget, Mode, PodInfo};
use crate::grid::{CellSnapshot, CursorStyle, Snapshot, TermColor, TerminalGrid, xterm_color};
use crate::input::{self, KeyInput, KeyModes, MouseReport};
use crate::settings::TerminalSettings;
use crate::shell;

/// Key context of a terminal. Control keys are bound here so the app's and gpui-component's
/// bindings (`ctrl-c` copy, `tab` focus, `ctrl-p` palette on Linux…) don't swallow them.
pub(crate) const CONTEXT: &str = "TerminalView";

actions!(
    terminal,
    [
        Copy,
        Paste,
        SelectAll,
        ClearSelection,
        ScrollPageUp,
        ScrollPageDown,
        ScrollToTop,
        ScrollToBottom,
        /// Starts the session again after it ended.
        Reconnect,
    ]
);

/// Sends a keystroke to the program instead of letting a binding handle it.
#[derive(Clone, PartialEq, Debug, Deserialize, JsonSchema, Action)]
#[action(namespace = terminal)]
pub struct SendKeystroke(pub String);

/// Keys the terminal keeps for itself.
pub(crate) fn init_keys(cx: &mut App) {
    let context = Some(CONTEXT);
    let mut bindings = Vec::new();
    let mut send = |keys: String| {
        bindings.push(KeyBinding::new(&keys, SendKeystroke(keys.clone()), context));
    };
    for c in 'a'..='z' {
        send(format!("ctrl-{c}"));
    }
    for key in [
        "ctrl-[",
        "ctrl-]",
        "ctrl-\\",
        "ctrl-space",
        "ctrl-/",
        "ctrl-@",
        "tab",
        "shift-tab",
        "escape",
        "enter",
        "alt-left",
        "alt-right",
        "alt-up",
        "alt-down",
        "alt-backspace",
        "alt-b",
        "alt-f",
        "alt-d",
        "alt-.",
    ] {
        send(key.to_string());
    }
    cx.bind_keys(bindings);
    let (copy, paste) = if cfg!(target_os = "macos") {
        ("cmd-c", "cmd-v")
    } else {
        ("ctrl-shift-c", "ctrl-shift-v")
    };
    cx.bind_keys([
        KeyBinding::new(copy, Copy, context),
        KeyBinding::new(paste, Paste, context),
        KeyBinding::new("shift-insert", Paste, context),
        KeyBinding::new("secondary-a", SelectAll, context),
        KeyBinding::new("shift-pageup", ScrollPageUp, context),
        KeyBinding::new("shift-pagedown", ScrollPageDown, context),
        KeyBinding::new("shift-home", ScrollToTop, context),
        KeyBinding::new("shift-end", ScrollToBottom, context),
    ]);
}

/// What a terminal session runs.
#[derive(Clone, Debug, PartialEq)]
pub enum SessionMode {
    /// A shell (`None`: detect `/bin/bash` → `/bin/sh` → `sh`, or the settings override).
    Exec { shell: Option<String> },
    /// The container's main process (`kubectl attach`).
    Attach,
    /// A new ephemeral container (`kubectl debug`), then attached.
    Debug(DebugSpec),
    /// A privileged pod on the node with a shell in the host's namespaces. `target` is the
    /// node.
    NodeShell { image: String, namespace: String },
}

/// A terminal to open: the pod (or node), container and session mode.
#[derive(Clone, Debug, PartialEq)]
pub struct TerminalSpec {
    pub target: ResourceRef,
    pub container: Option<String>,
    pub mode: SessionMode,
}

impl TerminalSpec {
    pub fn exec(target: ResourceRef) -> Self {
        Self {
            target,
            container: None,
            mode: SessionMode::Exec { shell: None },
        }
    }
}

#[derive(Clone, Debug, PartialEq)]
enum Status {
    NotConnected,
    Starting(SharedString),
    Connected,
    Ended(SharedString),
    Failed(SharedString),
    Stopped,
}

/// Where the grid was painted, to map the mouse to cells.
#[derive(Clone, Copy, Debug)]
struct Layout {
    origin: Point<Pixels>,
    cell: gpui::Size<Pixels>,
}

/// A node-shell pod to delete when the session ends.
struct NodePod {
    client: kube::Client,
    namespace: String,
    name: String,
}

impl NodePod {
    /// Deletes the pod on Tokio, detached (it must finish even after the view is gone).
    fn delete(self) {
        kubyl_core::runtime::handle().spawn(async move {
            exec::delete_pod(&self.client, &self.namespace, &self.name).await;
        });
    }
}

pub struct TerminalView {
    focus: FocusHandle,
    spec: TerminalSpec,
    /// Editor tabs of plain exec sessions come back on the next start; debug and node shells
    /// don't.
    restorable: bool,
    grid: TerminalGrid,
    columns: usize,
    rows: usize,
    layout: Option<Layout>,
    status: Status,
    /// Resolved container and command, for the header and the session row.
    container: Option<String>,
    command: Option<String>,
    /// Program title (OSC 0/2).
    title: Option<String>,
    pod_info: Option<PodInfo>,
    input_tx: Option<mpsc::UnboundedSender<Vec<u8>>>,
    resize_tx: Option<mpsc::UnboundedSender<(u16, u16)>>,
    session_id: Option<SessionId>,
    node_pod: Option<NodePod>,
    selecting: bool,
    /// Mouse button held while the program gets mouse reports.
    reporting_button: Option<u8>,
    last_report_cell: Option<(usize, usize)>,
    hovered_link: Option<(usize, Range<usize>, String)>,
    scroll_remainder: f32,
    /// Waits for the cluster to connect (a tab restored at startup).
    connect_subscription: Option<Subscription>,
    _task: Option<Task<()>>,
    _subscriptions: Vec<Subscription>,
}

impl TerminalView {
    /// A plain exec shell for a restored or `OpenView` editor tab.
    pub fn from_request(target: Option<ResourceRef>, cx: &mut Context<Self>) -> Self {
        let target = target.unwrap_or_else(|| {
            ResourceRef::object(
                ClusterId::new(""),
                kubyl_core::Gvr::new("", "v1", "pods"),
                None,
                String::new(),
            )
        });
        Self::new(TerminalSpec::exec(target), true, cx)
    }

    pub fn new(spec: TerminalSpec, restorable: bool, cx: &mut Context<Self>) -> Self {
        let settings = kubyl_settings::Settings::get::<TerminalSettings>(cx).clone();
        let (columns, rows) = (80, 24);
        let mut this = Self {
            focus: cx.focus_handle(),
            restorable: restorable && matches!(spec.mode, SessionMode::Exec { .. }),
            container: spec.container.clone(),
            spec,
            grid: TerminalGrid::new(columns, rows, settings.scrollback_lines),
            columns,
            rows,
            layout: None,
            status: Status::Starting("connecting…".into()),
            command: None,
            title: None,
            pod_info: None,
            input_tx: None,
            resize_tx: None,
            session_id: None,
            node_pod: None,
            selecting: false,
            reporting_button: None,
            last_report_cell: None,
            hovered_link: None,
            scroll_remainder: 0.0,
            connect_subscription: None,
            _task: None,
            _subscriptions: Vec::new(),
        };
        let release = cx.on_release(|this, cx| {
            // The tab was closed: drop the session row and the node-shell pod (the exec task
            // itself stops when `this` is dropped).
            if let Some(id) = this.session_id.take() {
                SessionRegistry::remove(cx, id);
            }
            if let Some(pod) = this.node_pod.take() {
                pod.delete();
            }
        });
        this._subscriptions.push(release);
        if this
            .spec
            .target
            .name
            .as_deref()
            .is_some_and(|n| !n.is_empty())
        {
            this.start(cx);
        } else {
            this.status = Status::Failed("nothing to connect to".into());
        }
        this
    }

    pub fn spec(&self) -> &TerminalSpec {
        &self.spec
    }

    /// `exec · x2kqp/api · /bin/sh`, for tabs and the session row.
    pub fn title(&self) -> String {
        let name = self.spec.target.name.clone().unwrap_or_default();
        let short = kubyl_logs::line::short_pod_name(&name).to_string();
        let target = match &self.container {
            Some(container) => format!("{short}/{container}"),
            None => short,
        };
        match &self.spec.mode {
            SessionMode::Exec { .. } => match &self.command {
                Some(command) => format!("exec · {target} · {command}"),
                None => format!("exec · {target}"),
            },
            SessionMode::Attach => format!("attach · {target}"),
            SessionMode::Debug(spec) => format!("debug · {target} · {}", spec.image),
            SessionMode::NodeShell { .. } => format!("node shell · {name}"),
        }
    }

    fn set_status(&mut self, status: Status, cx: &mut Context<Self>) {
        self.status = status;
        if let Some(id) = self.session_id {
            let (label, tone) = self.status_label();
            SessionRegistry::set_status(cx, id, label, tone);
            SessionRegistry::set_title(cx, id, self.title());
        }
        cx.notify();
    }

    fn status_label(&self) -> (SharedString, Tone) {
        match &self.status {
            Status::NotConnected => ("not connected".into(), Tone::Muted),
            Status::Starting(step) => (step.clone(), Tone::Info),
            Status::Connected => {
                let mut label = "websocket".to_string();
                if let Some(command) = &self.command {
                    label = format!("{command} · {label}");
                }
                (label.into(), Tone::Good)
            }
            Status::Ended(reason) => (reason.clone(), Tone::Muted),
            Status::Failed(err) => (format!("error: {err}").into(), Tone::Bad),
            Status::Stopped => ("stopped".into(), Tone::Muted),
        }
    }

    fn ensure_session(&mut self, cx: &mut Context<Self>) {
        if self.session_id.is_some() {
            return;
        }
        let weak = cx.weak_entity();
        let subtitle: SharedString = match &self.spec.mode {
            SessionMode::NodeShell { namespace, .. } => namespace.clone().into(),
            _ => self
                .spec
                .target
                .namespace
                .clone()
                .unwrap_or_default()
                .into(),
        };
        let id = SessionRegistry::add(
            cx,
            SessionKind::Terminal,
            self.title(),
            subtitle,
            "connecting…",
            Tone::Info,
            move |cx| {
                // Dropping the task drops the exec websocket (the Task-drop rule).
                weak.update(cx, |this, cx| {
                    this._task = None;
                    this.session_id = None;
                    this.input_tx = None;
                    this.resize_tx = None;
                    if let Some(pod) = this.node_pod.take() {
                        pod.delete();
                    }
                    this.status = Status::Stopped;
                    cx.notify();
                })
                .ok();
            },
        );
        self.session_id = Some(id);
    }

    fn start(&mut self, cx: &mut Context<Self>) {
        let Some(client) = ConnectionManager::global(cx)
            .read(cx)
            .client(&self.spec.target.cluster)
        else {
            self.status = Status::NotConnected;
            self.wait_for_cluster(cx);
            return;
        };
        self.connect_subscription = None;
        self.ensure_session(cx);
        let settings = kubyl_settings::Settings::get::<TerminalSettings>(cx).clone();
        let spec = self.spec.clone();
        let namespace = spec.target.namespace.clone().unwrap_or_default();
        let name = spec.target.name.clone().unwrap_or_default();

        let (input_tx, input_rx) = mpsc::unbounded();
        let (output_tx, mut output_rx) = mpsc::unbounded();
        let (resize_tx, resize_rx) = mpsc::unbounded();
        // The current size goes first, so the program starts with the right dimensions.
        resize_tx
            .unbounded_send((self.columns as u16, self.rows as u16))
            .ok();
        self.input_tx = Some(input_tx);
        self.resize_tx = Some(resize_tx);

        self._task = Some(cx.spawn(async move |this, cx| {
            let step = |this: &gpui::WeakEntity<Self>, cx: &mut gpui::AsyncApp, text: &str| {
                let text: SharedString = text.to_string().into();
                this.update(cx, |this, cx| this.set_status(Status::Starting(text), cx))
                    .is_ok()
            };
            // Prepare the pod/container/command off the UI thread.
            let setup_client = client.clone();
            let prepared = match &spec.mode {
                SessionMode::NodeShell { image, namespace } => {
                    if !step(&this, cx, "starting a node-shell pod…") {
                        return;
                    }
                    let (image, pod_namespace) = (image.clone(), namespace.clone());
                    let node = name.clone();
                    let task = cx.update(|cx| {
                        kubyl_core::spawn_kube(cx, async move {
                            exec::create_node_shell(&setup_client, &pod_namespace, &node, &image)
                                .await
                                .map(|pod| (pod_namespace, pod))
                        })
                    });
                    match task.await {
                        Ok((pod_namespace, pod)) => {
                            this.update(cx, |this, _| {
                                this.node_pod = Some(NodePod {
                                    client: client.clone(),
                                    namespace: pod_namespace.clone(),
                                    name: pod.clone(),
                                });
                                this.container = Some("shell".into());
                                this.command = Some("nsenter".into());
                            })
                            .ok();
                            Ok(ExecTarget {
                                namespace: pod_namespace,
                                pod,
                                container: Some("shell".into()),
                                mode: Mode::Exec {
                                    command: exec::node_shell_command(),
                                },
                            })
                        }
                        Err(err) => Err(format!("{err:#}")),
                    }
                }
                SessionMode::Debug(debug) => {
                    if !step(&this, cx, &format!("starting {}…", debug.image)) {
                        return;
                    }
                    let (debug, ns, pod) = (debug.clone(), namespace.clone(), name.clone());
                    let task = cx.update(|cx| {
                        kubyl_core::spawn_kube(cx, async move {
                            exec::create_debug_container(&setup_client, &ns, &pod, &debug).await
                        })
                    });
                    match task.await {
                        Ok(container) => {
                            this.update(cx, |this, _| this.container = Some(container.clone()))
                                .ok();
                            Ok(ExecTarget {
                                namespace: namespace.clone(),
                                pod: name.clone(),
                                container: Some(container),
                                mode: Mode::Attach {
                                    tty: true,
                                    stdin: true,
                                },
                            })
                        }
                        Err(err) => Err(format!("{err:#}")),
                    }
                }
                SessionMode::Exec { .. } | SessionMode::Attach => {
                    let (ns, pod) = (namespace.clone(), name.clone());
                    let pinned = spec.container.clone();
                    let exec_shell = match &spec.mode {
                        SessionMode::Exec { shell } => {
                            Some(shell.clone().or(settings.shell_override.clone()))
                        }
                        _ => None,
                    };
                    let task = cx.update(|cx| {
                        kubyl_core::spawn_kube(cx, async move {
                            let info = exec::pod_info(&setup_client, &ns, &pod).await?;
                            let container = pinned.or(info.default.clone());
                            let command = match exec_shell {
                                Some(shell) => Some(
                                    shell::detect(
                                        &setup_client,
                                        &ns,
                                        &pod,
                                        container.as_deref(),
                                        shell.as_deref(),
                                    )
                                    .await,
                                ),
                                None => None,
                            };
                            anyhow::Ok((info, container, command))
                        })
                    });
                    match task.await {
                        Ok((info, container, command)) => {
                            let mode = match &command {
                                Some(shell) => Mode::Exec {
                                    command: vec![shell.clone()],
                                },
                                None => {
                                    let c = container.as_deref().and_then(|c| info.get(c));
                                    Mode::Attach {
                                        tty: c.is_some_and(|c| c.tty),
                                        stdin: c.is_some_and(|c| c.stdin),
                                    }
                                }
                            };
                            this.update(cx, |this, _| {
                                this.pod_info = Some(info);
                                this.container = container.clone();
                                this.command = command;
                            })
                            .ok();
                            Ok(ExecTarget {
                                namespace: namespace.clone(),
                                pod: name.clone(),
                                container,
                                mode,
                            })
                        }
                        Err(err) => Err(format!("{err:#}")),
                    }
                }
            };
            let target = match prepared {
                Ok(target) => target,
                Err(err) => {
                    this.update(cx, |this, cx| {
                        this.set_status(Status::Failed(err.into()), cx)
                    })
                    .ok();
                    return;
                }
            };
            if let Mode::Attach { stdin: false, .. } = target.mode {
                this.update(cx, |this, cx| {
                    this.grid
                        .advance(b"\x1b[2m(the container has no stdin: output only)\x1b[0m\r\n");
                    cx.notify();
                })
                .ok();
            }

            let (connected_tx, connected_rx) = futures::channel::oneshot::channel();
            let _run = cx.update(|cx| {
                kubyl_core::spawn_kube(cx, async move {
                    exec::run(client, target, input_rx, output_tx, resize_rx, connected_tx).await
                })
            });
            match connected_rx.await {
                Ok(Ok(())) => {
                    this.update(cx, |this, cx| this.set_status(Status::Connected, cx))
                        .ok();
                }
                Ok(Err(message)) => {
                    this.update(cx, |this, cx| {
                        this.set_status(Status::Failed(message.into()), cx)
                    })
                    .ok();
                    return;
                }
                Err(_) => {
                    this.update(cx, |this, cx| {
                        this.set_status(Status::Ended("disconnected".into()), cx)
                    })
                    .ok();
                    return;
                }
            }

            while let Some(bytes) = output_rx.next().await {
                let mut batch = bytes;
                while let Ok(more) = output_rx.try_recv() {
                    batch.extend_from_slice(&more);
                }
                let alive = this.update(cx, |this, cx| this.receive(&batch, cx)).is_ok();
                if !alive {
                    break;
                }
                cx.background_executor()
                    .timer(Duration::from_millis(8))
                    .await;
            }
            this.update(cx, |this, cx| {
                this.input_tx = None;
                if let Some(pod) = this.node_pod.take() {
                    pod.delete();
                }
                this.set_status(
                    Status::Ended("session ended · press Enter to reconnect".into()),
                    cx,
                );
            })
            .ok();
        }));
    }

    /// Connects the cluster (a tab restored at startup opens before it) and starts the session
    /// once it is up.
    fn wait_for_cluster(&mut self, cx: &mut Context<Self>) {
        if self.connect_subscription.is_some() {
            return;
        }
        let manager = ConnectionManager::global(cx);
        let cluster = self.spec.target.cluster.clone();
        manager.update(cx, |manager, cx| manager.ensure_connected(&cluster, cx));
        self.connect_subscription = Some(cx.subscribe(
            &manager,
            move |this, manager, event: &ConnectionEvent, cx| {
                if let ConnectionEvent::StateChanged(id) = event
                    && *id == cluster
                    && manager.read(cx).state(id).is_connected()
                    && this.status == Status::NotConnected
                {
                    this.status = Status::Starting("connecting…".into());
                    this.start(cx);
                    cx.notify();
                }
            },
        ));
    }

    fn receive(&mut self, bytes: &[u8], cx: &mut Context<Self>) {
        self.grid.advance(bytes);
        let replies = self.grid.take_pty_writes();
        if !replies.is_empty() {
            self.send(replies);
        }
        if let Some(title) = self.grid.take_title() {
            self.title = title;
        }
        cx.notify();
    }

    fn reconnect(&mut self, cx: &mut Context<Self>) {
        if matches!(self.status, Status::Connected | Status::Starting(_)) {
            return;
        }
        self._task = None;
        self.grid
            .advance(b"\r\n\x1b[2m-- reconnecting --\x1b[0m\r\n");
        self.status = Status::Starting("connecting…".into());
        self.start(cx);
        cx.notify();
    }

    fn send(&self, bytes: Vec<u8>) {
        if let Some(tx) = &self.input_tx {
            tx.unbounded_send(bytes).ok();
        }
    }

    fn key_modes(&self, cx: &App) -> KeyModes {
        KeyModes {
            app_cursor: self.grid.modes().app_cursor,
            alt_is_meta: kubyl_settings::Settings::get::<TerminalSettings>(cx).option_as_meta,
        }
    }

    fn type_key(&mut self, key: KeyInput, cx: &mut Context<Self>) {
        if self.input_tx.is_none() {
            if key.key == "enter" {
                self.reconnect(cx);
            }
            return;
        }
        if let Some(bytes) = input::encode(&key, self.key_modes(cx)) {
            self.grid.scroll_to_bottom();
            self.grid.clear_selection();
            self.send(bytes);
            cx.notify();
        }
    }

    fn handle_key(&mut self, event: &KeyDownEvent, cx: &mut Context<Self>) {
        let ks = &event.keystroke;
        let key = KeyInput {
            key: ks.key.clone(),
            shift: ks.modifiers.shift,
            control: ks.modifiers.control,
            alt: ks.modifiers.alt,
            platform: ks.modifiers.platform,
            key_char: ks.key_char.clone(),
        };
        if key.platform {
            return;
        }
        cx.stop_propagation();
        self.type_key(key, cx);
    }

    fn paste(&mut self, cx: &mut Context<Self>) {
        let Some(text) = cx.read_from_clipboard().and_then(|item| item.text()) else {
            return;
        };
        let bracketed = self.grid.modes().bracketed_paste;
        self.grid.scroll_to_bottom();
        self.send(input::paste(&text, bracketed));
        cx.notify();
    }

    fn copy(&mut self, cx: &mut Context<Self>) {
        if let Some(text) = self.grid.selection_text() {
            cx.write_to_clipboard(ClipboardItem::new_string(text));
        }
    }

    /// Resizes the grid (and the remote TTY) to what fits the painted area.
    fn fit(&mut self, columns: usize, rows: usize) {
        if columns == self.columns && rows == self.rows {
            return;
        }
        self.columns = columns;
        self.rows = rows;
        self.grid.resize(columns, rows);
        if let Some(tx) = &self.resize_tx {
            tx.unbounded_send((columns as u16, rows as u16)).ok();
        }
    }

    /// The cell under a window position, and whether it's on the cell's right half.
    fn cell_at(&self, position: Point<Pixels>) -> Option<(usize, usize, bool)> {
        let layout = self.layout?;
        let x = f32::from(position.x - layout.origin.x);
        let y = f32::from(position.y - layout.origin.y);
        let (w, h) = (f32::from(layout.cell.width), f32::from(layout.cell.height));
        let column = (x / w).max(0.0);
        let row = (y / h).max(0.0) as usize;
        let right_half = column.fract() > 0.5;
        Some((
            row.min(self.rows.saturating_sub(1)),
            (column as usize).min(self.columns.saturating_sub(1)),
            right_half,
        ))
    }

    fn report_modifiers(modifiers: &Modifiers) -> u8 {
        4 * modifiers.shift as u8 + 8 * modifiers.alt as u8 + 16 * modifiers.control as u8
    }

    fn mouse_down(&mut self, event: &MouseDownEvent, window: &mut Window, cx: &mut Context<Self>) {
        self.focus.focus(window, cx);
        let Some((row, column, right)) = self.cell_at(event.position) else {
            return;
        };
        let modes = self.grid.modes();
        let button = match event.button {
            MouseButton::Left => 0,
            MouseButton::Middle => 1,
            MouseButton::Right => 2,
            _ => return,
        };
        if modes.mouse && !event.modifiers.shift && self.input_tx.is_some() {
            if let Some(bytes) = input::mouse_report(
                MouseReport::Press { button },
                column,
                row,
                Self::report_modifiers(&event.modifiers),
                modes.sgr_mouse,
            ) {
                self.send(bytes);
            }
            self.reporting_button = Some(button);
            self.last_report_cell = Some((row, column));
            return;
        }
        if button != 0 {
            return;
        }
        if event.modifiers.secondary()
            && let Some((_, url)) = self.grid.link_at(row, column)
        {
            cx.open_url(&url);
            return;
        }
        self.grid
            .start_selection(row, column, right, event.click_count);
        self.selecting = true;
        cx.notify();
    }

    fn mouse_move(&mut self, event: &MouseMoveEvent, cx: &mut Context<Self>) {
        let Some((row, column, right)) = self.cell_at(event.position) else {
            return;
        };
        if let Some(button) = self.reporting_button {
            let modes = self.grid.modes();
            if (modes.mouse_drag || modes.mouse_motion)
                && self.last_report_cell != Some((row, column))
            {
                if let Some(bytes) = input::mouse_report(
                    MouseReport::Drag {
                        button: Some(button),
                    },
                    column,
                    row,
                    Self::report_modifiers(&event.modifiers),
                    modes.sgr_mouse,
                ) {
                    self.send(bytes);
                }
                self.last_report_cell = Some((row, column));
            }
            return;
        }
        if self.selecting && event.pressed_button == Some(MouseButton::Left) {
            self.grid.update_selection(row, column, right);
            cx.notify();
            return;
        }
        let link = if event.modifiers.secondary() {
            self.grid
                .link_at(row, column)
                .map(|(cols, url)| (row, cols, url))
        } else {
            None
        };
        if link != self.hovered_link {
            self.hovered_link = link;
            cx.notify();
        }
    }

    fn mouse_up(&mut self, event: &MouseUpEvent, cx: &mut Context<Self>) {
        if let Some(button) = self.reporting_button.take() {
            if let Some((row, column, _)) = self.cell_at(event.position) {
                let modes = self.grid.modes();
                if let Some(bytes) = input::mouse_report(
                    MouseReport::Release { button },
                    column,
                    row,
                    Self::report_modifiers(&event.modifiers),
                    modes.sgr_mouse,
                ) {
                    self.send(bytes);
                }
            }
            return;
        }
        if self.selecting {
            self.selecting = false;
            if !self.grid.has_selection() {
                self.grid.clear_selection();
            }
            cx.notify();
        }
    }

    fn scroll(&mut self, event: &ScrollWheelEvent, cx: &mut Context<Self>) {
        let Some(layout) = self.layout else { return };
        let delta = event.delta.pixel_delta(layout.cell.height);
        self.scroll_remainder += f32::from(delta.y);
        let line = f32::from(layout.cell.height);
        let lines = (self.scroll_remainder / line).trunc() as i32;
        if lines == 0 {
            return;
        }
        self.scroll_remainder -= lines as f32 * line;
        let modes = self.grid.modes();
        if modes.mouse && self.input_tx.is_some() && !event.modifiers.shift {
            if let Some((row, column, _)) = self.cell_at(event.position) {
                let report = if lines > 0 {
                    MouseReport::WheelUp
                } else {
                    MouseReport::WheelDown
                };
                for _ in 0..lines.unsigned_abs().min(10) {
                    if let Some(bytes) =
                        input::mouse_report(report, column, row, 0, modes.sgr_mouse)
                    {
                        self.send(bytes);
                    }
                }
            }
            return;
        }
        if modes.alt_screen && modes.alternate_scroll && self.input_tx.is_some() {
            let key = if lines > 0 { "up" } else { "down" };
            let bytes = input::encode(
                &KeyInput {
                    key: key.into(),
                    ..Default::default()
                },
                self.key_modes(cx),
            )
            .unwrap_or_default();
            for _ in 0..lines.unsigned_abs().min(20) {
                self.send(bytes.clone());
            }
            return;
        }
        self.grid.scroll(lines);
        cx.notify();
    }

    fn render_header(&self, cx: &mut Context<Self>) -> gpui::AnyElement {
        let colors = cx.colors().clone();
        let (status, tone) = self.status_label();
        let weak = cx.weak_entity();
        let containers: Vec<String> = self
            .pod_info
            .as_ref()
            .map(|i| i.containers.iter().map(|c| c.name.clone()).collect())
            .unwrap_or_default();
        let current = self.container.clone();
        let container_menu = (containers.len() > 1
            && matches!(
                self.spec.mode,
                SessionMode::Exec { .. } | SessionMode::Attach
            ))
        .then(|| {
            let label: SharedString = current.clone().unwrap_or_default().into();
            let weak = weak.clone();
            MenuButton::new("terminal-container")
                .ghost()
                .compact()
                .child(
                    h_flex()
                        .gap(u(4.0))
                        .text_size(u(11.5))
                        .child(Icon::new(IconName::Box).size(11.0))
                        .child(label)
                        .child(Icon::new(IconName::ChevronDown).size(10.0)),
                )
                .dropdown_menu(move |menu, _, _| {
                    let mut menu = menu;
                    for name in &containers {
                        let weak = weak.clone();
                        let pick = name.clone();
                        menu = menu.item(
                            PopupMenuItem::new(name.clone())
                                .checked(current.as_deref() == Some(name))
                                .on_click(move |_, _, cx| {
                                    let pick = pick.clone();
                                    weak.update(cx, |this, cx| {
                                        this.spec.container = Some(pick);
                                        this.restart(cx);
                                    })
                                    .ok();
                                }),
                        );
                    }
                    menu
                })
        });
        let shell_menu = matches!(self.spec.mode, SessionMode::Exec { .. }).then(|| {
            let label: SharedString = self
                .command
                .clone()
                .unwrap_or_else(|| "shell".into())
                .into();
            let weak = weak.clone();
            let chosen = match &self.spec.mode {
                SessionMode::Exec { shell } => shell.clone(),
                _ => None,
            };
            MenuButton::new("terminal-shell")
                .ghost()
                .compact()
                .child(
                    h_flex()
                        .gap(u(4.0))
                        .text_size(u(11.5))
                        .font_family(fonts::MONO)
                        .child(label)
                        .child(Icon::new(IconName::ChevronDown).size(10.0)),
                )
                .dropdown_menu(move |menu, _, _| {
                    let mut menu = menu;
                    let options: [(&str, Option<&str>); 6] = [
                        ("Detect", None),
                        ("/bin/bash", Some("/bin/bash")),
                        ("/bin/sh", Some("/bin/sh")),
                        ("/bin/ash", Some("/bin/ash")),
                        ("/bin/zsh", Some("/bin/zsh")),
                        ("sh", Some("sh")),
                    ];
                    for (label, shell) in options {
                        let weak = weak.clone();
                        let shell = shell.map(String::from);
                        menu =
                            menu.item(PopupMenuItem::new(label).checked(chosen == shell).on_click(
                                move |_, _, cx| {
                                    let shell = shell.clone();
                                    weak.update(cx, |this, cx| {
                                        this.spec.mode = SessionMode::Exec { shell };
                                        this.restart(cx);
                                    })
                                    .ok();
                                },
                            ));
                    }
                    menu
                })
        });
        let ended = matches!(
            self.status,
            Status::Ended(_) | Status::Failed(_) | Status::Stopped | Status::NotConnected
        );
        h_flex()
            .flex_none()
            .h(u(26.0))
            .px(u(8.0))
            .gap(u(6.0))
            .border_b_1()
            .border_color(colors.border_variant)
            .bg(colors.panel)
            .text_size(u(11.5))
            .text_color(colors.text_dim)
            .child(
                Icon::new(match self.spec.mode {
                    SessionMode::NodeShell { .. } => IconName::Server,
                    _ => IconName::Terminal,
                })
                .size(12.0)
                .color(colors.text_dim),
            )
            .child(
                div()
                    .flex_1()
                    .min_w_0()
                    .truncate()
                    .font_family(fonts::MONO)
                    .child(self.title.clone().unwrap_or_else(|| self.title())),
            )
            .children(container_menu)
            .children(shell_menu)
            .child(kubyl_ui::StatusDot::new(kubyl_ui::tone_color(
                tone, &colors,
            )))
            .child(div().max_w(u(320.0)).truncate().child(status))
            .when(ended, |this| {
                this.child(
                    IconButton::new("terminal-reconnect", IconName::RefreshCw)
                        .icon_size(12.0)
                        .on_click(cx.listener(|this, _, _, cx| this.reconnect(cx))),
                )
            })
            .into_any_element()
    }

    /// Starts the session over (container or shell changed).
    fn restart(&mut self, cx: &mut Context<Self>) {
        self._task = None;
        self.input_tx = None;
        self.resize_tx = None;
        self.grid.advance(b"\x1b[2J\x1b[H");
        self.status = Status::Starting("connecting…".into());
        self.start(cx);
        cx.notify();
    }
}

/// The 16 ANSI colors from the theme (bright variants a bit lighter).
fn ansi_color(index: u8, colors: &Colors) -> Hsla {
    let base = match index % 8 {
        0 => colors.border,
        1 => colors.red,
        2 => colors.green,
        3 => colors.yellow,
        4 => colors.accent,
        5 => colors.purple,
        6 => colors.cyan,
        _ => colors.text_muted,
    };
    if index < 8 {
        return base;
    }
    match index {
        8 => colors.text_faint,
        15 => colors.text,
        _ => Hsla {
            l: (base.l + 0.08).min(0.95),
            ..base
        },
    }
}

fn rgb(r: u8, g: u8, b: u8) -> Hsla {
    gpui::rgb(((r as u32) << 16) | ((g as u32) << 8) | b as u32).into()
}

/// Resolves a cell color against the theme and the program's palette changes.
fn resolve_color(
    color: TermColor,
    fg: bool,
    palette: &[Option<(u8, u8, u8)>],
    colors: &Colors,
) -> Hsla {
    match color {
        TermColor::Default => {
            if fg {
                colors.text
            } else {
                colors.background
            }
        }
        TermColor::Named(n) => match palette.get(n as usize).copied().flatten() {
            Some((r, g, b)) => rgb(r, g, b),
            None => ansi_color(n, colors),
        },
        TermColor::Indexed(i) => {
            let (r, g, b) = palette
                .get(i as usize)
                .copied()
                .flatten()
                .unwrap_or_else(|| xterm_color(i));
            rgb(r, g, b)
        }
        TermColor::Rgb(r, g, b) => rgb(r, g, b),
    }
}

/// Foreground and background of a cell after inverse, dim and selection.
fn cell_colors(
    cell: &CellSnapshot,
    palette: &[Option<(u8, u8, u8)>],
    colors: &Colors,
) -> (Hsla, Hsla) {
    let mut fg = resolve_color(cell.fg, true, palette, colors);
    let mut bg = resolve_color(cell.bg, false, palette, colors);
    if cell.style.inverse {
        std::mem::swap(&mut fg, &mut bg);
    }
    if cell.style.dim {
        fg = fg.opacity(0.66);
    }
    if cell.selected {
        bg = colors.selection;
        if cell.style.inverse {
            fg = colors.text;
        }
    }
    (fg, bg)
}

/// Everything the paint pass needs.
struct Frame {
    snapshot: Snapshot,
    origin: Point<Pixels>,
    cell: gpui::Size<Pixels>,
    font: Font,
    font_size: Pixels,
    focused: bool,
    link: Option<(usize, Range<usize>)>,
    colors: Colors,
}

fn paint_frame(bounds: Bounds<Pixels>, frame: Frame, window: &mut Window, cx: &mut App) {
    let Frame {
        snapshot,
        origin,
        cell,
        font,
        font_size,
        focused,
        link,
        colors,
    } = frame;
    window.paint_quad(fill(bounds, colors.background));
    let palette = &snapshot.palette;
    for (row_ix, row) in snapshot.rows.iter().enumerate() {
        let y = origin.y + cell.height * row_ix as f32;
        // Backgrounds, merged into runs.
        let mut column = 0;
        while column < row.len() {
            let (_, bg) = cell_colors(&row[column], palette, &colors);
            let start = column;
            column += 1;
            while column < row.len() && cell_colors(&row[column], palette, &colors).1 == bg {
                column += 1;
            }
            if bg != colors.background {
                let x = origin.x + cell.width * start as f32;
                window.paint_quad(fill(
                    Bounds::new(
                        point(x, y),
                        size(cell.width * (column - start) as f32, cell.height),
                    ),
                    bg,
                ));
            }
        }
        // Text, in runs of equal style (wide characters on their own).
        let mut column = 0;
        while column < row.len() {
            let first = &row[column];
            if first.spacer {
                column += 1;
                continue;
            }
            let (fg, _) = cell_colors(first, palette, &colors);
            let style = first.style;
            let start = column;
            let mut text = String::new();
            let width_cells = if first.wide {
                text.push(if style.hidden { ' ' } else { first.c });
                column += 2;
                2
            } else {
                while column < row.len() {
                    let c = &row[column];
                    if c.spacer
                        || c.wide
                        || c.style != style
                        || cell_colors(c, palette, &colors).0 != fg
                    {
                        break;
                    }
                    text.push(if style.hidden || c.c == '\0' {
                        ' '
                    } else {
                        c.c
                    });
                    column += 1;
                }
                1
            };
            let decorated = style.underline || style.strikeout;
            if text.trim().is_empty() && !decorated {
                continue;
            }
            let run_font = Font {
                weight: if style.bold {
                    FontWeight::BOLD
                } else {
                    FontWeight::NORMAL
                },
                style: if style.italic {
                    FontStyle::Italic
                } else {
                    FontStyle::Normal
                },
                ..font.clone()
            };
            let run = TextRun {
                len: text.len(),
                font: run_font,
                color: fg,
                background_color: None,
                underline: style.underline.then_some(UnderlineStyle {
                    thickness: px(1.0),
                    color: Some(fg),
                    wavy: false,
                }),
                strikethrough: style.strikeout.then_some(StrikethroughStyle {
                    thickness: px(1.0),
                    color: Some(fg),
                }),
            };
            let shaped = window.text_system().shape_line(
                text.into(),
                font_size,
                &[run],
                Some(cell.width * width_cells as f32),
            );
            let x = origin.x + cell.width * start as f32;
            shaped
                .paint(point(x, y), cell.height, TextAlign::Left, None, window, cx)
                .ok();
        }
        // Cmd/Ctrl-hovered link.
        if let Some((link_row, cols)) = &link
            && *link_row == row_ix
        {
            let x = origin.x + cell.width * cols.start as f32;
            window.paint_quad(fill(
                Bounds::new(
                    point(x, y + cell.height - px(2.0)),
                    size(cell.width * cols.len() as f32, px(1.0)),
                ),
                colors.accent,
            ));
        }
    }
    // Cursor.
    if let Some(cursor) = snapshot.cursor {
        let x = origin.x + cell.width * cursor.column as f32;
        let y = origin.y + cell.height * cursor.row as f32;
        let under = snapshot
            .rows
            .get(cursor.row)
            .and_then(|r| r.get(cursor.column));
        let wide = under.is_some_and(|c| c.wide);
        let width = cell.width * if wide { 2.0 } else { 1.0 };
        let block = Bounds::new(point(x, y), size(width, cell.height));
        let style = if focused {
            cursor.style
        } else {
            CursorStyle::HollowBlock
        };
        match style {
            CursorStyle::Block => {
                window.paint_quad(fill(block, colors.accent));
                if let Some(under) = under
                    && under.c != ' '
                {
                    let run = TextRun {
                        len: under.c.len_utf8(),
                        font: font.clone(),
                        color: colors.background,
                        background_color: None,
                        underline: None,
                        strikethrough: None,
                    };
                    let shaped = window.text_system().shape_line(
                        under.c.to_string().into(),
                        font_size,
                        &[run],
                        Some(width),
                    );
                    shaped
                        .paint(point(x, y), cell.height, TextAlign::Left, None, window, cx)
                        .ok();
                }
            }
            CursorStyle::HollowBlock => {
                window.paint_quad(outline(block, colors.accent, BorderStyle::Solid));
            }
            CursorStyle::Beam => {
                window.paint_quad(fill(
                    Bounds::new(point(x, y), size(px(2.0), cell.height)),
                    colors.accent,
                ));
            }
            CursorStyle::Underline => {
                window.paint_quad(fill(
                    Bounds::new(point(x, y + cell.height - px(2.0)), size(width, px(2.0))),
                    colors.accent,
                ));
            }
        }
    }
    // Scrolled back: a small position hint.
    if snapshot.display_offset > 0 {
        let label = format!("↑ {} of {}", snapshot.display_offset, snapshot.history);
        let run = TextRun {
            len: label.len(),
            font: font.clone(),
            color: colors.text_dim,
            background_color: Some(colors.elevated),
            underline: None,
            strikethrough: None,
        };
        let shaped = window
            .text_system()
            .shape_line(label.into(), font_size * 0.85, &[run], None);
        let x = bounds.origin.x + bounds.size.width - shaped.width - px(12.0);
        shaped
            .paint(
                point(x, bounds.origin.y + px(4.0)),
                cell.height,
                TextAlign::Left,
                None,
                window,
                cx,
            )
            .ok();
    }
}

impl Focusable for TerminalView {
    fn focus_handle(&self, _: &App) -> FocusHandle {
        self.focus.clone()
    }
}

impl TabView for TerminalView {
    fn tab_title(&self, _: &App) -> SharedString {
        self.title().into()
    }

    fn tab_icon(&self, _: &App) -> Option<SharedString> {
        Some(IconName::Terminal.path())
    }

    fn view_request(&self, _: &App) -> Option<ViewRequest> {
        self.restorable.then(|| {
            ViewRequest::for_resource(kubyl_core::ViewKind::Terminal, self.spec.target.clone())
        })
    }
}

impl Render for TerminalView {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let colors = cx.colors().clone();
        let focused = self.focus.is_focused(window);
        let weak = cx.weak_entity();
        let settings_size = kubyl_settings::Settings::get::<TerminalSettings>(cx).font_size;
        let header = self.render_header(cx);
        let link = self
            .hovered_link
            .as_ref()
            .map(|(row, cols, _)| (*row, cols.clone()));
        let pointer = link.is_some();
        let prepaint_colors = colors.clone();

        div()
            .id("terminal")
            .key_context(CONTEXT)
            .track_focus(&self.focus)
            .flex()
            .flex_col()
            .size_full()
            .bg(colors.background)
            .on_action(cx.listener(|this, action: &SendKeystroke, _, cx| {
                this.type_key(KeyInput::parse(&action.0), cx)
            }))
            .on_action(cx.listener(|this, _: &Copy, _, cx| this.copy(cx)))
            .on_action(cx.listener(|this, _: &Paste, _, cx| this.paste(cx)))
            .on_action(cx.listener(|this, _: &SelectAll, _, cx| {
                this.grid.select_all();
                cx.notify();
            }))
            .on_action(cx.listener(|this, _: &ClearSelection, _, cx| {
                this.grid.clear_selection();
                cx.notify();
            }))
            .on_action(cx.listener(|this, _: &ScrollPageUp, _, cx| {
                this.grid.scroll_page(true);
                cx.notify();
            }))
            .on_action(cx.listener(|this, _: &ScrollPageDown, _, cx| {
                this.grid.scroll_page(false);
                cx.notify();
            }))
            .on_action(cx.listener(|this, _: &ScrollToTop, _, cx| {
                this.grid.scroll_to_top();
                cx.notify();
            }))
            .on_action(cx.listener(|this, _: &ScrollToBottom, _, cx| {
                this.grid.scroll_to_bottom();
                cx.notify();
            }))
            .on_action(cx.listener(|this, _: &Reconnect, _, cx| this.reconnect(cx)))
            .on_key_down(
                cx.listener(|this, event: &KeyDownEvent, _, cx| this.handle_key(event, cx)),
            )
            .child(header)
            .child(
                div()
                    .id("terminal-grid")
                    .relative()
                    .flex_1()
                    .min_h_0()
                    .overflow_hidden()
                    .when(pointer, |this| this.cursor_pointer())
                    .when(!pointer, |this| this.cursor_text())
                    .on_mouse_down(
                        MouseButton::Left,
                        cx.listener(|this, event, window, cx| this.mouse_down(event, window, cx)),
                    )
                    .on_mouse_down(
                        MouseButton::Middle,
                        cx.listener(|this, event, window, cx| this.mouse_down(event, window, cx)),
                    )
                    .on_mouse_down(
                        MouseButton::Right,
                        cx.listener(|this, event, window, cx| this.mouse_down(event, window, cx)),
                    )
                    .on_mouse_move(cx.listener(|this, event, _, cx| this.mouse_move(event, cx)))
                    .on_mouse_up(
                        MouseButton::Left,
                        cx.listener(|this, event, _, cx| this.mouse_up(event, cx)),
                    )
                    .on_mouse_up(
                        MouseButton::Middle,
                        cx.listener(|this, event, _, cx| this.mouse_up(event, cx)),
                    )
                    .on_mouse_up(
                        MouseButton::Right,
                        cx.listener(|this, event, _, cx| this.mouse_up(event, cx)),
                    )
                    .on_scroll_wheel(cx.listener(|this, event, _, cx| this.scroll(event, cx)))
                    .child(
                        canvas(
                            move |bounds, window, cx| {
                                let font = gpui::font(fonts::MONO);
                                let font_size = window.rem_size() * (settings_size / 16.0);
                                let text_system = window.text_system();
                                let font_id = text_system.resolve_font(&font);
                                let cell_width = text_system
                                    .advance(font_id, font_size, 'm')
                                    .map(|s| s.width)
                                    .unwrap_or(font_size * 0.6);
                                let cell = size(cell_width, (font_size * 1.35).round());
                                let padding = window.rem_size() * (6.0 / 16.0);
                                let origin = bounds.origin + point(padding, padding);
                                let inner_w = f32::from(bounds.size.width - padding * 2.0);
                                let inner_h = f32::from(bounds.size.height - padding * 2.0);
                                let columns =
                                    (inner_w / f32::from(cell.width)).floor().max(2.0) as usize;
                                let rows =
                                    (inner_h / f32::from(cell.height)).floor().max(2.0) as usize;
                                let snapshot = weak
                                    .update(cx, |this, _| {
                                        this.layout = Some(Layout { origin, cell });
                                        this.fit(columns, rows);
                                        this.grid.snapshot()
                                    })
                                    .ok();
                                snapshot.map(|snapshot| Frame {
                                    snapshot,
                                    origin,
                                    cell,
                                    font,
                                    font_size,
                                    focused,
                                    link,
                                    colors: prepaint_colors,
                                })
                            },
                            |bounds, frame, window, cx| {
                                if let Some(frame) = frame {
                                    paint_frame(bounds, frame, window, cx);
                                }
                            },
                        )
                        .size_full(),
                    ),
            )
    }
}

/// Shows an error toast (for sessions that couldn't start before a view existed).
pub(crate) fn notify_error(cx: &mut App, message: impl Into<SharedString>) {
    NotificationCenter::push(cx, Notification::error(message));
}
