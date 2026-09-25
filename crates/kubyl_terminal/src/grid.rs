//! Terminal state: an `alacritty_terminal::Term` driven by raw PTY bytes, with a pure
//! `snapshot()` the view renders through GPUI's text system (no glyph-atlas/GPU renderer of our
//! own — see the phase 05 handoff log for what that would take).

use std::cell::RefCell;
use std::collections::VecDeque;
use std::rc::Rc;

use alacritty_terminal::event::{Event, EventListener};
use alacritty_terminal::grid::Dimensions as _;
use alacritty_terminal::index::{Column, Line};
use alacritty_terminal::term::cell::Flags;
use alacritty_terminal::term::test::TermSize;
use alacritty_terminal::term::{Config, Term};
use alacritty_terminal::vte::ansi::{Color as AnsiColor, NamedColor, Processor, Rgb};

/// Ignores most terminal-driven events (title changes, clipboard, bell…) but buffers
/// `Event::PtyWrite` bytes — the replies to terminal query sequences (cursor position DSR,
/// device attributes, color queries) that programs like vim/fish block on. The view drains
/// these with [`TerminalGrid::take_pty_writes`] and forwards them back to the PTY's stdin.
#[derive(Clone, Default)]
pub struct PtyWriteListener(Rc<RefCell<VecDeque<Vec<u8>>>>);

impl EventListener for PtyWriteListener {
    fn send_event(&self, event: Event) {
        if let Event::PtyWrite(text) = event {
            self.0.borrow_mut().push_back(text.into_bytes());
        }
    }
}

/// One cell of the rendered grid, in a form the GPUI view can turn into text runs without
/// touching `alacritty_terminal` types.
#[derive(Clone, Debug, PartialEq)]
pub struct CellSnapshot {
    pub c: char,
    pub fg: TermColor,
    pub bg: TermColor,
    pub bold: bool,
    pub italic: bool,
    pub underline: bool,
}

/// A resolved or palette-relative color. The view resolves `Named`/`Indexed` against the theme
/// so 256-color and true-color output both work without `kubyl_terminal` depending on
/// `kubyl_ui`'s color choices.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum TermColor {
    Default,
    Named(u8),
    Indexed(u8),
    Rgb(u8, u8, u8),
}

impl From<AnsiColor> for TermColor {
    fn from(color: AnsiColor) -> Self {
        match color {
            AnsiColor::Named(NamedColor::Foreground | NamedColor::Background) => TermColor::Default,
            AnsiColor::Named(named) => TermColor::Named(named as u8),
            AnsiColor::Spec(Rgb { r, g, b }) => TermColor::Rgb(r, g, b),
            AnsiColor::Indexed(i) => TermColor::Indexed(i),
        }
    }
}

pub struct Snapshot {
    pub columns: usize,
    pub rows: Vec<Vec<CellSnapshot>>,
    pub cursor: (usize, usize),
}

/// Wraps `Term<PtyWriteListener>` plus the ANSI parser that feeds it.
pub struct TerminalGrid {
    term: Term<PtyWriteListener>,
    parser: Processor,
    pty_writes: PtyWriteListener,
}

impl TerminalGrid {
    pub fn new(columns: usize, rows: usize) -> Self {
        let size = TermSize::new(columns.max(1), rows.max(1));
        let pty_writes = PtyWriteListener::default();
        let term = Term::new(Config::default(), &size, pty_writes.clone());
        Self {
            term,
            parser: Processor::new(),
            pty_writes,
        }
    }

    /// Feeds raw bytes read from the exec/attach stream into the terminal state machine.
    pub fn advance(&mut self, bytes: &[u8]) {
        self.parser.advance(&mut self.term, bytes);
    }

    /// Drains any PTY reply bytes queued by terminal query sequences (cursor position, device
    /// attributes, color queries) since the last call, ready to write back to the PTY's stdin.
    pub fn take_pty_writes(&mut self) -> Vec<u8> {
        let mut queue = self.pty_writes.0.borrow_mut();
        let mut out = Vec::new();
        while let Some(bytes) = queue.pop_front() {
            out.extend_from_slice(&bytes);
        }
        out
    }

    pub fn resize(&mut self, columns: usize, rows: usize) {
        self.term.resize(TermSize::new(columns.max(1), rows.max(1)));
    }

    pub fn columns(&self) -> usize {
        self.term.columns()
    }

    pub fn screen_lines(&self) -> usize {
        self.term.screen_lines()
    }

    /// A snapshot of the visible grid, ready to render.
    pub fn snapshot(&self) -> Snapshot {
        let columns = self.term.columns();
        let screen_lines = self.term.screen_lines();
        let grid = self.term.grid();
        let mut rows = Vec::with_capacity(screen_lines);
        for line in 0..screen_lines {
            let row = &grid[Line(line as i32)];
            let mut cells = Vec::with_capacity(columns);
            for col in 0..columns {
                let cell = &row[Column(col)];
                cells.push(CellSnapshot {
                    c: cell.c,
                    fg: cell.fg.into(),
                    bg: cell.bg.into(),
                    bold: cell.flags.contains(Flags::BOLD),
                    italic: cell.flags.contains(Flags::ITALIC),
                    underline: cell.flags.intersects(
                        Flags::UNDERLINE
                            | Flags::DOUBLE_UNDERLINE
                            | Flags::UNDERCURL
                            | Flags::DOTTED_UNDERLINE
                            | Flags::DASHED_UNDERLINE,
                    ),
                });
            }
            rows.push(cells);
        }
        let cursor = grid.cursor.point;
        Snapshot {
            columns,
            rows,
            cursor: (cursor.line.0.max(0) as usize, cursor.column.0),
        }
    }

    /// The full visible text (for copy/select-all), one line per row, trailing spaces trimmed.
    pub fn text(&self) -> String {
        self.snapshot()
            .rows
            .iter()
            .map(|row| {
                row.iter()
                    .map(|c| c.c)
                    .collect::<String>()
                    .trim_end()
                    .to_string()
            })
            .collect::<Vec<_>>()
            .join("\n")
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn plain_text_lands_at_the_cursor() {
        let mut grid = TerminalGrid::new(20, 5);
        grid.advance(b"hello");
        let snap = grid.snapshot();
        let line: String = snap.rows[0].iter().map(|c| c.c).collect();
        assert!(line.starts_with("hello"));
        assert_eq!(snap.cursor, (0, 5));
    }

    #[test]
    fn newline_and_carriage_return_move_the_cursor() {
        let mut grid = TerminalGrid::new(20, 5);
        grid.advance(b"one\r\ntwo");
        let snap = grid.snapshot();
        let line0: String = snap.rows[0].iter().map(|c| c.c).collect();
        let line1: String = snap.rows[1].iter().map(|c| c.c).collect();
        assert!(line0.starts_with("one"));
        assert!(line1.starts_with("two"));
    }

    #[test]
    fn sgr_sets_true_color_foreground() {
        let mut grid = TerminalGrid::new(20, 5);
        grid.advance(b"\x1b[38;2;10;20;30mx");
        let snap = grid.snapshot();
        assert_eq!(snap.rows[0][0].fg, TermColor::Rgb(10, 20, 30));
    }

    #[test]
    fn sgr_sets_named_and_indexed_colors() {
        let mut grid = TerminalGrid::new(20, 5);
        grid.advance(b"\x1b[31mred\x1b[0m\x1b[38;5;200mindexed");
        let snap = grid.snapshot();
        assert_eq!(snap.rows[0][0].fg, TermColor::Named(1));
        assert_eq!(snap.rows[0][3].fg, TermColor::Indexed(200));
    }

    #[test]
    fn bold_flag_is_tracked() {
        let mut grid = TerminalGrid::new(20, 5);
        grid.advance(b"\x1b[1mbold");
        let snap = grid.snapshot();
        assert!(snap.rows[0][0].bold);
    }

    #[test]
    fn resize_changes_dimensions() {
        let mut grid = TerminalGrid::new(20, 5);
        grid.resize(40, 10);
        assert_eq!(grid.columns(), 40);
        assert_eq!(grid.screen_lines(), 10);
    }

    #[test]
    fn cursor_movement_sequences_are_applied() {
        let mut grid = TerminalGrid::new(20, 5);
        // Move to row 3, column 5 (1-based in CUP).
        grid.advance(b"\x1b[3;5Hx");
        let snap = grid.snapshot();
        assert_eq!(snap.rows[2][4].c, 'x');
    }
}
