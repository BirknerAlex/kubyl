//! `ViewKind::Terminal`: the exec terminal view (board 2 · Live logs, exec shell).
//!
//! Renders [`crate::grid::TerminalGrid`] as rows of GPUI text (one text run per contiguous
//! same-style cell range), which satisfies "GPUI renderer via the text system" without a
//! from-scratch glyph atlas. Keyboard input goes through [`crate::input::encode`]; the exec
//! websocket runs off the UI thread via [`crate::exec::run`].

use std::time::Duration;

use futures::StreamExt as _;
use futures::channel::mpsc;
use gpui::{
    App, Bounds, Context, FocusHandle, Focusable, IntoElement, KeyDownEvent, Pixels, SharedString,
    Subscription, Task, Window, actions, canvas, div, prelude::*,
};
use kubyl_core::{ClusterId, ResourceRef, TabView, Tone, ViewRequest};
use kubyl_kube::ConnectionManager;
use kubyl_logs::sessions::{SessionId, SessionKind, SessionRegistry};
use kubyl_ui::{ActiveColors, Colors, IconName, fonts, u};

use crate::exec::{self, ExecTarget, Mode};
use crate::grid::{TermColor, TerminalGrid};
use crate::input::{self, KeyInput};
use crate::settings::TerminalSettings;
use crate::shell;

pub(crate) const CONTEXT: &str = "TerminalView";

actions!(terminal, [CopyAll]);

/// Approximate IBM Plex Mono cell metrics at the terminal's font size. Exact glyph metrics come
/// from GPUI's text system at paint time in a full implementation; this first pass uses a fixed
/// advance, which is close enough for common terminal font sizes.
const CHAR_WIDTH: f32 = 7.8;
const LINE_HEIGHT: f32 = 18.0;

pub struct TerminalView {
    focus: FocusHandle,
    request: ViewRequest,
    target: ResourceRef,
    grid: TerminalGrid,
    columns: usize,
    rows: usize,
    status: SharedString,
    input_tx: Option<mpsc::UnboundedSender<Vec<u8>>>,
    resize_tx: Option<mpsc::UnboundedSender<(u16, u16)>>,
    session_id: Option<SessionId>,
    _task: Option<Task<()>>,
    _subscriptions: Vec<Subscription>,
}

impl TerminalView {
    pub fn new(target: Option<ResourceRef>, cx: &mut Context<Self>) -> Self {
        let target = target.unwrap_or_else(|| {
            ResourceRef::object(
                ClusterId::new(""),
                kubyl_core::Gvr::new("", "v1", "pods"),
                None,
                String::new(),
            )
        });
        let columns = 80;
        let rows = 24;
        let mut this = Self {
            focus: cx.focus_handle(),
            request: ViewRequest::for_resource(kubyl_core::ViewKind::Terminal, target.clone()),
            target,
            grid: TerminalGrid::new(columns, rows),
            columns,
            rows,
            status: "connecting…".into(),
            input_tx: None,
            resize_tx: None,
            session_id: None,
            _task: None,
            _subscriptions: Vec::new(),
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
        let pod = self.target.name.clone().unwrap_or_default();
        let shell_override = kubyl_settings::Settings::get::<TerminalSettings>(cx)
            .shell_override
            .clone();
        let shell = shell::pick(shell_override.as_deref(), |_| false);

        let (input_tx, input_rx) = mpsc::unbounded();
        let (output_tx, output_rx) = mpsc::unbounded();
        let (resize_tx, resize_rx) = mpsc::unbounded();
        self.input_tx = Some(input_tx);
        self.resize_tx = Some(resize_tx);

        let title = format!("Shell: {pod}");
        let subtitle: SharedString = namespace.clone().into();
        let target = ExecTarget {
            namespace,
            pod,
            container: None,
            mode: Mode::Exec {
                command: vec![shell],
            },
        };
        self._task = Some(cx.spawn(async move |this, cx| {
            let _run_task = cx.update(|cx| {
                kubyl_core::spawn_kube(cx, async move {
                    exec::run(client, target, input_rx, output_tx, resize_rx).await
                })
            });
            this.update(cx, |this, cx| {
                let id = SessionRegistry::add(
                    cx,
                    SessionKind::Terminal,
                    title,
                    subtitle,
                    "connected",
                    Tone::Good,
                    |_| {},
                );
                this.session_id = Some(id);
                this.status = "connected".into();
                cx.notify();
            })
            .ok();
            let mut output_rx = output_rx;
            while let Some(bytes) = output_rx.next().await {
                let mut batch = bytes;
                while let Ok(more) = output_rx.try_recv() {
                    batch.extend_from_slice(&more);
                }
                let alive = this
                    .update(cx, |this, cx| {
                        this.grid.advance(&batch);
                        cx.notify();
                    })
                    .is_ok();
                if !alive {
                    break;
                }
                cx.background_executor()
                    .timer(Duration::from_millis(16))
                    .await;
            }
            this.update(cx, |this, cx| {
                this.status = "disconnected".into();
                if let Some(id) = this.session_id {
                    SessionRegistry::set_status(cx, id, "disconnected", Tone::Muted);
                }
                cx.notify();
            })
            .ok();
        }));
    }

    fn handle_key(&mut self, event: &KeyDownEvent, cx: &mut Context<Self>) {
        let Some(tx) = &self.input_tx else { return };
        let ks = &event.keystroke;
        let key = KeyInput {
            key: ks.key.clone(),
            shift: ks.modifiers.shift,
            control: ks.modifiers.control,
            alt: ks.modifiers.alt,
            ime_key: ks.key_char.clone(),
        };
        if let Some(bytes) = input::encode(&key) {
            tx.unbounded_send(bytes).ok();
        }
        cx.notify();
    }

    fn maybe_resize(&mut self, bounds: Bounds<Pixels>, cx: &mut Context<Self>) {
        let columns = ((f32::from(bounds.size.width) / CHAR_WIDTH) as usize).max(2);
        let rows = ((f32::from(bounds.size.height) / LINE_HEIGHT) as usize).max(2);
        if columns == self.columns && rows == self.rows {
            return;
        }
        self.columns = columns;
        self.rows = rows;
        self.grid.resize(columns, rows);
        if let Some(tx) = &self.resize_tx {
            tx.unbounded_send((columns as u16, rows as u16)).ok();
        }
        cx.notify();
    }
}

impl Focusable for TerminalView {
    fn focus_handle(&self, _: &App) -> FocusHandle {
        self.focus.clone()
    }
}

impl TabView for TerminalView {
    fn tab_title(&self, _: &App) -> SharedString {
        format!("Shell: {}", self.target.name.clone().unwrap_or_default()).into()
    }

    fn tab_icon(&self, _: &App) -> Option<SharedString> {
        Some(IconName::Terminal.path())
    }

    fn view_request(&self, _: &App) -> Option<ViewRequest> {
        Some(self.request.clone())
    }
}

fn resolve_color(color: TermColor, colors: &Colors, fg: bool) -> gpui::Hsla {
    match color {
        TermColor::Default => {
            if fg {
                colors.text
            } else {
                colors.background
            }
        }
        TermColor::Named(n) => named_color(n, colors),
        TermColor::Indexed(i) if i < 16 => named_color(i, colors),
        TermColor::Indexed(i) => {
            // A coarse 256-color -> theme mapping: fall back to the true-color value alacritty
            // resolved, approximated through the named ANSI ramp by index modulo 16 so at least
            // something reasonable shows without a full 256-color palette table.
            named_color(i % 16, colors)
        }
        TermColor::Rgb(r, g, b) => {
            gpui::rgb(((r as u32) << 16) | ((g as u32) << 8) | b as u32).into()
        }
    }
}

fn named_color(index: u8, colors: &Colors) -> gpui::Hsla {
    match index % 16 {
        0 => colors.background,
        1 => colors.red,
        2 => colors.green,
        3 => colors.yellow,
        4 => colors.accent,
        5 => colors.purple,
        6 => colors.cyan,
        7 => colors.text,
        8 => colors.text_faint,
        9 => colors.red,
        10 => colors.green,
        11 => colors.yellow,
        12 => colors.accent,
        13 => colors.purple,
        14 => colors.cyan,
        _ => colors.text,
    }
}

impl Render for TerminalView {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let colors = cx.colors().clone();
        let snapshot = self.grid.snapshot();
        let status = self.status.clone();
        let weak = cx.entity().downgrade();

        div()
            .id("terminal")
            .key_context(CONTEXT)
            .track_focus(&self.focus)
            .size_full()
            .bg(colors.background)
            .on_action(cx.listener(|this, _: &CopyAll, _, cx| {
                cx.write_to_clipboard(gpui::ClipboardItem::new_string(this.grid.text()));
            }))
            .on_key_down(
                cx.listener(|this, event: &KeyDownEvent, _, cx| this.handle_key(event, cx)),
            )
            .child(
                div()
                    .flex()
                    .flex_col()
                    .size_full()
                    .font_family(fonts::MONO)
                    .text_size(u(13.0))
                    .p(u(4.0))
                    .children(snapshot.rows.iter().enumerate().map(|(row_ix, row)| {
                        render_row(row, row_ix == snapshot.cursor.0, snapshot.cursor.1, &colors)
                    })),
            )
            .child(
                div()
                    .absolute()
                    .bottom_0()
                    .right_0()
                    .p(u(4.0))
                    .text_size(u(10.0))
                    .text_color(colors.text_faint)
                    .child(status),
            )
            .child(canvas(
                move |bounds, _, cx| {
                    weak.update(cx, |this, cx| {
                        this.maybe_resize(bounds, cx);
                    })
                    .ok();
                },
                |_, _, _, _| {},
            ))
    }
}

fn render_row(
    row: &[crate::grid::CellSnapshot],
    cursor_here: bool,
    cursor_col: usize,
    colors: &Colors,
) -> impl IntoElement {
    let mut spans: Vec<(String, gpui::Hsla, gpui::Hsla, bool)> = Vec::new();
    for (col, cell) in row.iter().enumerate() {
        let fg = resolve_color(cell.fg, colors, true);
        let mut bg = resolve_color(cell.bg, colors, false);
        if cursor_here && col == cursor_col {
            bg = colors.selection;
        }
        match spans.last_mut() {
            Some((text, last_fg, last_bg, bold))
                if *last_fg == fg && *last_bg == bg && *bold == cell.bold =>
            {
                text.push(cell.c);
            }
            _ => spans.push((cell.c.to_string(), fg, bg, cell.bold)),
        }
    }
    div()
        .flex()
        .flex_row()
        .children(spans.into_iter().map(|(text, fg, bg, bold)| {
            div()
                .text_color(fg)
                .bg(bg)
                .when(bold, |d| d.font_weight(gpui::FontWeight::BOLD))
                .child(text)
        }))
}
