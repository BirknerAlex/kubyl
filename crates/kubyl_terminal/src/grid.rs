//! Terminal state: an `alacritty_terminal::Term` driven by raw PTY bytes, and a [`Snapshot`] of
//! the visible viewport (scrollback offset, selection, cursor, modes) that the view paints on a
//! cell grid. Everything here is GPUI-free and unit-tested.

use std::cell::RefCell;
use std::collections::VecDeque;
use std::ops::Range;
use std::rc::Rc;
use std::sync::{Arc, LazyLock};

use alacritty_terminal::event::{Event, EventListener};
use alacritty_terminal::grid::{Dimensions as _, Scroll};
use alacritty_terminal::index::{Column, Line, Point, Side};
use alacritty_terminal::selection::{Selection, SelectionType};
use alacritty_terminal::term::cell::Flags;
use alacritty_terminal::term::test::TermSize;
use alacritty_terminal::term::{Config, Term, TermMode};
use alacritty_terminal::vte::ansi::{Color as AnsiColor, CursorShape, NamedColor, Processor, Rgb};
use regex::Regex;

/// Buffers what the terminal asks the host to do: `Event::PtyWrite` replies to query sequences
/// (cursor position, device attributes, colors) that programs like vim block on, and title
/// changes. The view drains them after each chunk of output.
#[derive(Clone, Default)]
pub struct Listener(Rc<RefCell<ListenerState>>);

#[derive(Default)]
struct ListenerState {
    writes: VecDeque<Vec<u8>>,
    title: Option<Option<String>>,
}

impl EventListener for Listener {
    fn send_event(&self, event: Event) {
        let mut state = self.0.borrow_mut();
        match event {
            Event::PtyWrite(text) => state.writes.push_back(text.into_bytes()),
            Event::Title(title) => state.title = Some(Some(title)),
            Event::ResetTitle => state.title = Some(None),
            _ => {}
        }
    }
}

/// A resolved or palette-relative color. The view resolves `Named`/`Indexed` against the theme
/// (and palette changes the program made with OSC 4).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum TermColor {
    /// The default foreground or background.
    Default,
    /// One of the 16 ANSI colors (0-7 normal, 8-15 bright).
    Named(u8),
    /// A 256-color palette index (16-231 color cube, 232-255 grays).
    Indexed(u8),
    Rgb(u8, u8, u8),
}

impl From<AnsiColor> for TermColor {
    fn from(color: AnsiColor) -> Self {
        match color {
            AnsiColor::Named(named) => {
                let n = named as usize;
                if n < 16 {
                    TermColor::Named(n as u8)
                } else if (NamedColor::DimBlack as usize..=NamedColor::DimWhite as usize)
                    .contains(&n)
                {
                    // Dim colors: the base color; the DIM flag fades it.
                    TermColor::Named((n - NamedColor::DimBlack as usize) as u8)
                } else {
                    TermColor::Default
                }
            }
            AnsiColor::Spec(Rgb { r, g, b }) => TermColor::Rgb(r, g, b),
            AnsiColor::Indexed(i) if i < 16 => TermColor::Named(i),
            AnsiColor::Indexed(i) => TermColor::Indexed(i),
        }
    }
}

/// The standard xterm 256-color palette entry for indices 16-255.
pub fn xterm_color(index: u8) -> (u8, u8, u8) {
    match index {
        16..=231 => {
            let i = index - 16;
            let level = |v: u8| if v == 0 { 0 } else { 55 + v * 40 };
            (level(i / 36), level((i / 6) % 6), level(i % 6))
        }
        232..=255 => {
            let v = 8 + (index - 232) * 10;
            (v, v, v)
        }
        // The 16 ANSI colors come from the theme; this is only a fallback.
        _ => (0x80, 0x80, 0x80),
    }
}

/// Cell attributes the renderer needs.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct CellStyle {
    pub bold: bool,
    pub italic: bool,
    pub underline: bool,
    pub strikeout: bool,
    pub dim: bool,
    pub inverse: bool,
    pub hidden: bool,
}

/// One cell of the rendered grid.
#[derive(Clone, Debug, PartialEq)]
pub struct CellSnapshot {
    pub c: char,
    pub fg: TermColor,
    pub bg: TermColor,
    pub style: CellStyle,
    /// The first half of a double-width character (it covers the next cell too).
    pub wide: bool,
    /// The second half of a double-width character (nothing to draw).
    pub spacer: bool,
    pub selected: bool,
    /// OSC 8 hyperlink target.
    pub link: Option<Arc<str>>,
}

impl Default for CellSnapshot {
    fn default() -> Self {
        Self {
            c: ' ',
            fg: TermColor::Default,
            bg: TermColor::Default,
            style: CellStyle::default(),
            wide: false,
            spacer: false,
            selected: false,
            link: None,
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum CursorStyle {
    Block,
    Underline,
    Beam,
    HollowBlock,
}

/// Where the cursor is in the viewport, if it is visible there.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct CursorSnapshot {
    pub row: usize,
    pub column: usize,
    pub style: CursorStyle,
}

/// Terminal modes that change how the view handles input.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Modes {
    pub app_cursor: bool,
    pub bracketed_paste: bool,
    /// The program wants mouse clicks (and maybe motion) reported.
    pub mouse: bool,
    pub mouse_motion: bool,
    pub mouse_drag: bool,
    /// SGR (1006) mouse encoding.
    pub sgr_mouse: bool,
    pub alt_screen: bool,
    /// Wheel scrolls send arrow keys on the alternate screen.
    pub alternate_scroll: bool,
    pub focus_reports: bool,
}

impl From<&TermMode> for Modes {
    fn from(mode: &TermMode) -> Self {
        Self {
            app_cursor: mode.contains(TermMode::APP_CURSOR),
            bracketed_paste: mode.contains(TermMode::BRACKETED_PASTE),
            mouse: mode.intersects(TermMode::MOUSE_MODE),
            mouse_motion: mode.contains(TermMode::MOUSE_MOTION),
            mouse_drag: mode.contains(TermMode::MOUSE_DRAG),
            sgr_mouse: mode.contains(TermMode::SGR_MOUSE),
            alt_screen: mode.contains(TermMode::ALT_SCREEN),
            alternate_scroll: mode.contains(TermMode::ALTERNATE_SCROLL),
            focus_reports: mode.contains(TermMode::FOCUS_IN_OUT),
        }
    }
}

/// The visible viewport.
#[derive(Clone, Debug)]
pub struct Snapshot {
    pub columns: usize,
    pub rows: Vec<Vec<CellSnapshot>>,
    pub cursor: Option<CursorSnapshot>,
    /// Lines scrolled back into history (0 = at the bottom).
    pub display_offset: usize,
    pub history: usize,
    pub modes: Modes,
    /// Palette entries the program changed (OSC 4), indexed like [`TermColor::Indexed`].
    pub palette: Vec<Option<(u8, u8, u8)>>,
}

/// Wraps `Term` plus the ANSI parser that feeds it.
pub struct TerminalGrid {
    term: Term<Listener>,
    parser: Processor,
    listener: Listener,
}

impl TerminalGrid {
    pub fn new(columns: usize, rows: usize, scrollback: usize) -> Self {
        let size = TermSize::new(columns.max(2), rows.max(2));
        let listener = Listener::default();
        let config = Config {
            scrolling_history: scrollback,
            ..Config::default()
        };
        let term = Term::new(config, &size, listener.clone());
        Self {
            term,
            parser: Processor::new(),
            listener,
        }
    }

    /// Feeds raw bytes read from the exec/attach stream into the terminal state machine.
    pub fn advance(&mut self, bytes: &[u8]) {
        self.parser.advance(&mut self.term, bytes);
    }

    /// Drains reply bytes queued by terminal query sequences since the last call, ready to
    /// write back to the PTY's stdin.
    pub fn take_pty_writes(&mut self) -> Vec<u8> {
        let mut state = self.listener.0.borrow_mut();
        let mut out = Vec::new();
        while let Some(bytes) = state.writes.pop_front() {
            out.extend_from_slice(&bytes);
        }
        out
    }

    /// A title change (OSC 0/2) since the last call: `Some(None)` = reset.
    pub fn take_title(&mut self) -> Option<Option<String>> {
        self.listener.0.borrow_mut().title.take()
    }

    pub fn resize(&mut self, columns: usize, rows: usize) {
        self.term.resize(TermSize::new(columns.max(2), rows.max(2)));
    }

    pub fn columns(&self) -> usize {
        self.term.columns()
    }

    pub fn screen_lines(&self) -> usize {
        self.term.screen_lines()
    }

    pub fn modes(&self) -> Modes {
        self.term.mode().into()
    }

    pub fn display_offset(&self) -> usize {
        self.term.grid().display_offset()
    }

    /// Scrolls the viewport through the scrollback (positive = up into history).
    pub fn scroll(&mut self, lines: i32) {
        self.term.scroll_display(Scroll::Delta(lines));
    }

    pub fn scroll_page(&mut self, up: bool) {
        self.term
            .scroll_display(if up { Scroll::PageUp } else { Scroll::PageDown });
    }

    pub fn scroll_to_bottom(&mut self) {
        self.term.scroll_display(Scroll::Bottom);
    }

    pub fn scroll_to_top(&mut self) {
        self.term.scroll_display(Scroll::Top);
    }

    /// The grid point of a viewport cell.
    fn point(&self, row: usize, column: usize) -> Point {
        let line = row as i32 - self.display_offset() as i32;
        let column = column.min(self.term.columns().saturating_sub(1));
        Point::new(Line(line), Column(column))
    }

    /// Starts a selection at a viewport cell. `right_half`: the pointer is on the cell's right
    /// half (the selection then starts after it). `clicks`: 1 = characters, 2 = words, 3 = lines.
    pub fn start_selection(&mut self, row: usize, column: usize, right_half: bool, clicks: usize) {
        let ty = match clicks {
            2 => SelectionType::Semantic,
            3.. => SelectionType::Lines,
            _ => SelectionType::Simple,
        };
        let side = if right_half { Side::Right } else { Side::Left };
        let point = self.point(row, column);
        self.term.selection = Some(Selection::new(ty, point, side));
    }

    pub fn update_selection(&mut self, row: usize, column: usize, right_half: bool) {
        let side = if right_half { Side::Right } else { Side::Left };
        let point = self.point(row, column);
        if let Some(selection) = &mut self.term.selection {
            selection.update(point, side);
        }
    }

    pub fn clear_selection(&mut self) {
        self.term.selection = None;
    }

    pub fn has_selection(&self) -> bool {
        self.term.selection.as_ref().is_some_and(|s| !s.is_empty())
    }

    pub fn selection_text(&self) -> Option<String> {
        self.term.selection_to_string().filter(|s| !s.is_empty())
    }

    /// Selects everything (history and screen).
    pub fn select_all(&mut self) {
        let top = self.term.topmost_line();
        let bottom = self.term.bottommost_line();
        let last = Column(self.term.columns().saturating_sub(1));
        let mut selection = Selection::new(
            SelectionType::Simple,
            Point::new(top, Column(0)),
            Side::Left,
        );
        selection.update(Point::new(bottom, last), Side::Right);
        self.term.selection = Some(selection);
    }

    /// A snapshot of the visible grid, ready to render.
    pub fn snapshot(&self) -> Snapshot {
        let content = self.term.renderable_content();
        let columns = self.term.columns();
        let screen_lines = self.term.screen_lines();
        let offset = content.display_offset as i32;
        let mut rows = vec![vec![CellSnapshot::default(); columns]; screen_lines];
        let selection = content.selection;
        for indexed in content.display_iter {
            let row = indexed.point.line.0 + offset;
            if row < 0 || row as usize >= screen_lines {
                continue;
            }
            let column = indexed.point.column.0;
            if column >= columns {
                continue;
            }
            let cell = indexed.cell;
            let flags = cell.flags;
            rows[row as usize][column] = CellSnapshot {
                c: cell.c,
                fg: cell.fg.into(),
                bg: cell.bg.into(),
                style: CellStyle {
                    bold: flags.contains(Flags::BOLD),
                    italic: flags.contains(Flags::ITALIC),
                    underline: flags.intersects(Flags::ALL_UNDERLINES),
                    strikeout: flags.contains(Flags::STRIKEOUT),
                    dim: flags.contains(Flags::DIM),
                    inverse: flags.contains(Flags::INVERSE),
                    hidden: flags.contains(Flags::HIDDEN),
                },
                wide: flags.contains(Flags::WIDE_CHAR),
                spacer: flags.intersects(Flags::WIDE_CHAR_SPACER | Flags::LEADING_WIDE_CHAR_SPACER),
                selected: selection.is_some_and(|s| s.contains(indexed.point)),
                link: cell.hyperlink().map(|link| Arc::from(link.uri())),
            };
        }
        let cursor = {
            let point = content.cursor.point;
            let row = point.line.0 + offset;
            let style = match content.cursor.shape {
                CursorShape::Hidden => None,
                CursorShape::Block => Some(CursorStyle::Block),
                CursorShape::Underline => Some(CursorStyle::Underline),
                CursorShape::Beam => Some(CursorStyle::Beam),
                CursorShape::HollowBlock => Some(CursorStyle::HollowBlock),
            };
            match style {
                Some(style) if row >= 0 && (row as usize) < screen_lines => Some(CursorSnapshot {
                    row: row as usize,
                    column: point.column.0.min(columns.saturating_sub(1)),
                    style,
                }),
                _ => None,
            }
        };
        let palette = (0..256)
            .map(|i| content.colors[i].map(|c| (c.r, c.g, c.b)))
            .collect();
        Snapshot {
            columns,
            rows,
            cursor,
            display_offset: content.display_offset,
            history: self.term.grid().history_size(),
            modes: (&content.mode).into(),
            palette,
        }
    }

    /// The visible text (for copy all), one line per row, trailing spaces trimmed.
    pub fn text(&self) -> String {
        self.snapshot()
            .rows
            .iter()
            .map(|row| row_text(row).0.trim_end().to_string())
            .collect::<Vec<_>>()
            .join("\n")
    }

    /// The link under a viewport cell: an OSC 8 hyperlink, or a URL in the row's text.
    pub fn link_at(&self, row: usize, column: usize) -> Option<(Range<usize>, String)> {
        let snapshot = self.snapshot();
        let cells = snapshot.rows.get(row)?;
        if let Some(link) = cells.get(column).and_then(|c| c.link.clone()) {
            // The contiguous run of cells with the same link.
            let start = (0..=column)
                .rev()
                .take_while(|&c| cells[c].link.as_deref() == Some(&*link))
                .last()
                .unwrap_or(column);
            let end = (column..cells.len())
                .take_while(|&c| cells[c].link.as_deref() == Some(&*link))
                .last()
                .unwrap_or(column);
            return Some((start..end + 1, link.to_string()));
        }
        let (text, columns) = row_text(cells);
        find_urls(&text)
            .into_iter()
            .map(|range| {
                let start = columns[text[..range.start].chars().count()];
                let chars = text[range.clone()].chars().count();
                let end = columns[text[..range.start].chars().count() + chars - 1] + 1;
                (start..end, text[range].to_string())
            })
            .find(|(cols, _)| cols.contains(&column))
    }
}

/// The text of a row and the column of each of its characters (wide-character spacers are
/// skipped).
pub fn row_text(cells: &[CellSnapshot]) -> (String, Vec<usize>) {
    let mut text = String::with_capacity(cells.len());
    let mut columns = Vec::with_capacity(cells.len());
    for (column, cell) in cells.iter().enumerate() {
        if cell.spacer {
            continue;
        }
        text.push(cell.c);
        columns.push(column);
    }
    (text, columns)
}

static URL: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r#"(?:https?|ftp|file)://[^\s<>"'`\x00-\x1f]+"#).expect("valid URL pattern")
});

/// Byte ranges of the URLs in `text`, without trailing punctuation.
pub fn find_urls(text: &str) -> Vec<Range<usize>> {
    URL.find_iter(text)
        .map(|m| {
            let trimmed = m
                .as_str()
                .trim_end_matches(['.', ',', ';', ':', '!', '?', ')', ']', '}', '\'']);
            m.start()..m.start() + trimmed.len()
        })
        .filter(|r| !r.is_empty())
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn line(snapshot: &Snapshot, row: usize) -> String {
        row_text(&snapshot.rows[row]).0
    }

    #[test]
    fn plain_text_lands_at_the_cursor() {
        let mut grid = TerminalGrid::new(20, 5, 100);
        grid.advance(b"hello");
        let snap = grid.snapshot();
        assert!(line(&snap, 0).starts_with("hello"));
        let cursor = snap.cursor.unwrap();
        assert_eq!((cursor.row, cursor.column), (0, 5));
    }

    #[test]
    fn newline_and_carriage_return_move_the_cursor() {
        let mut grid = TerminalGrid::new(20, 5, 100);
        grid.advance(b"one\r\ntwo");
        let snap = grid.snapshot();
        assert!(line(&snap, 0).starts_with("one"));
        assert!(line(&snap, 1).starts_with("two"));
    }

    #[test]
    fn sgr_colors_and_attributes() {
        let mut grid = TerminalGrid::new(40, 5, 100);
        grid.advance(b"\x1b[38;2;10;20;30mx\x1b[0m\x1b[31mr\x1b[38;5;200mi\x1b[1;3;4;7;9mz");
        let snap = grid.snapshot();
        let row = &snap.rows[0];
        assert_eq!(row[0].fg, TermColor::Rgb(10, 20, 30));
        assert_eq!(row[1].fg, TermColor::Named(1));
        assert_eq!(row[2].fg, TermColor::Indexed(200));
        let style = row[3].style;
        assert!(style.bold && style.italic && style.underline && style.inverse && style.strikeout);
    }

    #[test]
    fn bright_and_dim_colors_are_named() {
        let mut grid = TerminalGrid::new(20, 5, 100);
        grid.advance(b"\x1b[91mb\x1b[2;32md");
        let snap = grid.snapshot();
        assert_eq!(snap.rows[0][0].fg, TermColor::Named(9));
        assert_eq!(snap.rows[0][1].fg, TermColor::Named(2));
        assert!(snap.rows[0][1].style.dim);
    }

    #[test]
    fn xterm_palette() {
        assert_eq!(xterm_color(16), (0, 0, 0));
        assert_eq!(xterm_color(21), (0, 0, 255));
        assert_eq!(xterm_color(196), (255, 0, 0));
        assert_eq!(xterm_color(232), (8, 8, 8));
        assert_eq!(xterm_color(255), (238, 238, 238));
    }

    #[test]
    fn hidden_cursor_is_not_drawn() {
        let mut grid = TerminalGrid::new(20, 5, 100);
        grid.advance(b"\x1b[?25l");
        assert!(grid.snapshot().cursor.is_none());
        grid.advance(b"\x1b[?25h");
        assert!(grid.snapshot().cursor.is_some());
    }

    #[test]
    fn resize_changes_dimensions() {
        let mut grid = TerminalGrid::new(20, 5, 100);
        grid.resize(40, 10);
        assert_eq!(grid.columns(), 40);
        assert_eq!(grid.screen_lines(), 10);
    }

    #[test]
    fn cursor_movement_sequences_are_applied() {
        let mut grid = TerminalGrid::new(20, 5, 100);
        grid.advance(b"\x1b[3;5Hx");
        assert_eq!(grid.snapshot().rows[2][4].c, 'x');
    }

    #[test]
    fn scrollback_scrolls_into_history() {
        let mut grid = TerminalGrid::new(10, 3, 100);
        for i in 0..10 {
            grid.advance(format!("line{i}\r\n").as_bytes());
        }
        assert_eq!(grid.display_offset(), 0);
        grid.scroll(3);
        let snap = grid.snapshot();
        assert_eq!(snap.display_offset, 3);
        assert!(line(&snap, 0).starts_with("line5"));
        // The cursor is below the viewport while scrolled back.
        assert!(snap.cursor.is_none());
        grid.scroll_to_bottom();
        assert_eq!(grid.display_offset(), 0);
    }

    #[test]
    fn selection_copies_text() {
        let mut grid = TerminalGrid::new(20, 3, 100);
        grid.advance(b"hello world");
        grid.start_selection(0, 0, false, 1);
        grid.update_selection(0, 4, true);
        assert_eq!(grid.selection_text().as_deref(), Some("hello"));
        assert!(grid.snapshot().rows[0][2].selected);
        assert!(!grid.snapshot().rows[0][6].selected);
        // Double click selects a word.
        grid.start_selection(0, 8, false, 2);
        assert_eq!(grid.selection_text().as_deref(), Some("world"));
        grid.clear_selection();
        assert!(!grid.has_selection());
    }

    #[test]
    fn wide_characters_take_two_cells() {
        let mut grid = TerminalGrid::new(10, 2, 100);
        grid.advance("日x".as_bytes());
        let row = &grid.snapshot().rows[0];
        assert!(row[0].wide);
        assert!(row[1].spacer);
        assert_eq!(row[2].c, 'x');
        assert_eq!(row_text(row).0.trim_end(), "日x");
    }

    #[test]
    fn replies_to_terminal_queries_and_titles() {
        let mut grid = TerminalGrid::new(20, 5, 100);
        grid.advance(b"\x1b[6n\x1b]0;my title\x07");
        assert_eq!(grid.take_pty_writes(), b"\x1b[1;1R");
        assert_eq!(grid.take_title(), Some(Some("my title".into())));
        assert_eq!(grid.take_title(), None);
    }

    #[test]
    fn modes_follow_the_program() {
        let mut grid = TerminalGrid::new(20, 5, 100);
        grid.advance(b"\x1b[?1h\x1b[?2004h\x1b[?1000h\x1b[?1006h\x1b[?1049h");
        let modes = grid.modes();
        assert!(modes.app_cursor && modes.bracketed_paste && modes.mouse && modes.sgr_mouse);
        assert!(modes.alt_screen);
    }

    #[test]
    fn finds_urls_and_osc8_links() {
        assert_eq!(
            find_urls("see https://example.com/a?b=1, then http://x.io."),
            vec![4..29, 36..47]
        );
        let mut grid = TerminalGrid::new(40, 2, 100);
        grid.advance(b"go to https://kubyl.dev now");
        let (cols, url) = grid.link_at(0, 10).unwrap();
        assert_eq!(url, "https://kubyl.dev");
        assert_eq!(cols, 6..23);
        assert!(grid.link_at(0, 2).is_none());
        let mut grid = TerminalGrid::new(40, 2, 100);
        grid.advance(b"\x1b]8;;https://a.b/c\x1b\\click\x1b]8;;\x1b\\ here");
        let (cols, url) = grid.link_at(0, 1).unwrap();
        assert_eq!(url, "https://a.b/c");
        assert_eq!(cols, 0..5);
    }
}
