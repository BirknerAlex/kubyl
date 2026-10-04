//! Table columns and cells as plain data, for the UI and for anything that builds tables.

use std::convert::Infallible;

use crate::SharedString;
use crate::types::Tone;

/// How wide a column is.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum ColumnWidth {
    /// Fixed width in (unscaled) pixels.
    Fixed(f32),
    /// Share of the remaining width, like CSS `fr`, with a minimum in pixels.
    Flex { weight: f32, min: f32 },
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum Align {
    #[default]
    Start,
    End,
}

/// One table column.
#[derive(Clone, Debug, PartialEq)]
pub struct ColumnDef {
    pub id: SharedString,
    /// Header label; rendered upper-case.
    pub title: SharedString,
    pub width: ColumnWidth,
    pub align: Align,
    /// Render cells in the monospace font (names, numbers, ages).
    pub mono: bool,
    /// Only shown in the "wide" view (like `kubectl get -o wide`).
    pub wide: bool,
}

impl ColumnDef {
    pub fn new(
        id: impl Into<SharedString>,
        title: impl Into<SharedString>,
        width: ColumnWidth,
    ) -> Self {
        Self {
            id: id.into(),
            title: title.into(),
            width,
            align: Align::Start,
            mono: false,
            wide: false,
        }
    }

    pub fn mono(mut self) -> Self {
        self.mono = true;
        self
    }

    pub fn align_end(mut self) -> Self {
        self.align = Align::End;
        self
    }

    /// Hidden unless the view is in "wide" mode.
    pub fn wide(mut self) -> Self {
        self.wide = true;
        self
    }
}

/// A rendered cell value. `B` is the type of the buttons a UI puts in cells; cells made
/// without a UI have none (`Infallible`).
#[derive(Clone, Debug, PartialEq)]
pub enum CellValue<B = Infallible> {
    Text(SharedString),
    /// Text in a status color without a dot, e.g. a restart count in red.
    Tinted {
        label: SharedString,
        tone: Tone,
    },
    Status {
        label: SharedString,
        tone: Tone,
    },
    /// A value with a usage bar, e.g. CPU `184m` at 42%.
    Usage {
        label: SharedString,
        percent: f32,
    },
    /// Small buttons (one per HTTP port…); clicking one dispatches its action for the row.
    Buttons(Vec<B>),
    /// Renders as a faint dash.
    Empty,
}

impl<B> CellValue<B> {
    /// The same cell with its buttons turned into another type.
    pub fn map_buttons<C>(self, f: impl FnMut(B) -> C) -> CellValue<C> {
        match self {
            CellValue::Text(label) => CellValue::Text(label),
            CellValue::Tinted { label, tone } => CellValue::Tinted { label, tone },
            CellValue::Status { label, tone } => CellValue::Status { label, tone },
            CellValue::Usage { label, percent } => CellValue::Usage { label, percent },
            CellValue::Buttons(buttons) => CellValue::Buttons(buttons.into_iter().map(f).collect()),
            CellValue::Empty => CellValue::Empty,
        }
    }
}
