//! A virtualized table with fixed row height.
//!
//! Only visible rows are built (via `uniform_list`), so row count doesn't affect frame time.
//! The header stays in place while rows scroll. Fixed-width columns can be resized by dragging
//! the header edge; flex columns share the remaining width.

use std::ops::Range;

use gpui::{
    AnyElement, App, Context, ElementId, EventEmitter, FocusHandle, Focusable, IntoElement,
    KeyBinding, MouseButton, MouseDownEvent, MouseMoveEvent, Pixels, Render, ScrollStrategy,
    SharedString, UniformListScrollHandle, Window, actions, div, prelude::*, uniform_list,
};
use gpui_component::{h_flex, v_flex};
use kubyl_core::{Align, CellValue, ColumnDef, ColumnWidth};

use crate::{ActiveColors, ProgressBar, StatusPill, fonts, sizes, u};

actions!(
    data_table,
    [
        SelectNext,
        SelectPrevious,
        SelectFirst,
        SelectLast,
        SelectPageDown,
        SelectPageUp,
        Confirm,
    ]
);

const CONTEXT: &str = "DataTable";
const PAGE: usize = 20;
const MIN_COLUMN_WIDTH: f32 = 40.0;

pub(crate) fn init(cx: &mut App) {
    cx.bind_keys([
        KeyBinding::new("down", SelectNext, Some(CONTEXT)),
        KeyBinding::new("j", SelectNext, Some(CONTEXT)),
        KeyBinding::new("up", SelectPrevious, Some(CONTEXT)),
        KeyBinding::new("k", SelectPrevious, Some(CONTEXT)),
        KeyBinding::new("home", SelectFirst, Some(CONTEXT)),
        KeyBinding::new("g g", SelectFirst, Some(CONTEXT)),
        KeyBinding::new("end", SelectLast, Some(CONTEXT)),
        KeyBinding::new("shift-g", SelectLast, Some(CONTEXT)),
        KeyBinding::new("pagedown", SelectPageDown, Some(CONTEXT)),
        KeyBinding::new("ctrl-f", SelectPageDown, Some(CONTEXT)),
        KeyBinding::new("pageup", SelectPageUp, Some(CONTEXT)),
        KeyBinding::new("ctrl-b", SelectPageUp, Some(CONTEXT)),
        KeyBinding::new("enter", Confirm, Some(CONTEXT)),
    ]);
}

/// Supplies rows to a [`DataTable`]. Called only for visible rows.
pub trait TableDelegate: 'static {
    fn columns(&self) -> Vec<ColumnDef>;
    fn row_count(&self, cx: &App) -> usize;
    fn cell(&self, row: usize, column: usize, cx: &App) -> CellValue;
}

#[derive(Clone, Debug, PartialEq)]
pub enum DataTableEvent {
    SelectionChanged(Option<usize>),
    /// Enter or double-click on a row.
    Confirmed(usize),
}

struct Resize {
    column: usize,
    start_x: Pixels,
    start_width: f32,
}

pub struct DataTable {
    delegate: Box<dyn TableDelegate>,
    columns: Vec<ColumnDef>,
    selected: Option<usize>,
    row_height: f32,
    scroll: UniformListScrollHandle,
    focus: FocusHandle,
    resize: Option<Resize>,
}

impl EventEmitter<DataTableEvent> for DataTable {}

impl Focusable for DataTable {
    fn focus_handle(&self, _: &App) -> FocusHandle {
        self.focus.clone()
    }
}

impl DataTable {
    pub fn new(delegate: impl TableDelegate, cx: &mut Context<Self>) -> Self {
        let columns = delegate.columns();
        Self {
            delegate: Box::new(delegate),
            columns,
            selected: None,
            row_height: sizes::TABLE_ROW,
            scroll: UniformListScrollHandle::new(),
            focus: cx.focus_handle(),
            resize: None,
        }
    }

    /// Row height in unscaled pixels (default 30).
    pub fn row_height(mut self, px: f32) -> Self {
        self.row_height = px;
        self
    }

    pub fn delegate(&self) -> &dyn TableDelegate {
        self.delegate.as_ref()
    }

    pub fn columns(&self) -> &[ColumnDef] {
        &self.columns
    }

    pub fn selected(&self) -> Option<usize> {
        self.selected
    }

    /// Selects `row` (clamped to the row count) and scrolls it into view.
    pub fn select(&mut self, row: Option<usize>, cx: &mut Context<Self>) {
        let count = self.delegate.row_count(cx);
        let row = row.filter(|_| count > 0).map(|r| r.min(count - 1));
        if row == self.selected {
            return;
        }
        self.selected = row;
        if let Some(row) = row {
            self.scroll.scroll_to_item(row, ScrollStrategy::Nearest);
        }
        cx.emit(DataTableEvent::SelectionChanged(row));
        cx.notify();
    }

    fn move_selection(&mut self, delta: isize, cx: &mut Context<Self>) {
        let next = match self.selected {
            Some(row) => row.saturating_add_signed(delta),
            None => 0,
        };
        self.select(Some(next), cx);
    }

    fn select_next(&mut self, _: &SelectNext, _: &mut Window, cx: &mut Context<Self>) {
        self.move_selection(1, cx);
    }

    fn select_previous(&mut self, _: &SelectPrevious, _: &mut Window, cx: &mut Context<Self>) {
        self.move_selection(-1, cx);
    }

    fn select_first(&mut self, _: &SelectFirst, _: &mut Window, cx: &mut Context<Self>) {
        self.select(Some(0), cx);
    }

    fn select_last(&mut self, _: &SelectLast, _: &mut Window, cx: &mut Context<Self>) {
        self.select(Some(usize::MAX), cx);
    }

    fn select_page_down(&mut self, _: &SelectPageDown, _: &mut Window, cx: &mut Context<Self>) {
        self.move_selection(PAGE as isize, cx);
    }

    fn select_page_up(&mut self, _: &SelectPageUp, _: &mut Window, cx: &mut Context<Self>) {
        self.move_selection(-(PAGE as isize), cx);
    }

    fn confirm(&mut self, _: &Confirm, _: &mut Window, cx: &mut Context<Self>) {
        if let Some(row) = self.selected {
            cx.emit(DataTableEvent::Confirmed(row));
        }
    }

    fn start_resize(&mut self, column: usize, event: &MouseDownEvent, cx: &mut Context<Self>) {
        if let ColumnWidth::Fixed(width) = self.columns[column].width {
            self.resize = Some(Resize {
                column,
                start_x: event.position.x,
                start_width: width,
            });
            cx.stop_propagation();
        }
    }

    fn drag_resize(&mut self, event: &MouseMoveEvent, window: &mut Window, cx: &mut Context<Self>) {
        let Some(resize) = &self.resize else {
            return;
        };
        // Convert window pixels back to unscaled UI pixels.
        let scale = f32::from(window.rem_size()) / 16.0;
        let delta = f32::from(event.position.x - resize.start_x) / scale;
        let width = (resize.start_width + delta).max(MIN_COLUMN_WIDTH);
        self.columns[resize.column].width = ColumnWidth::Fixed(width);
        cx.notify();
    }

    fn render_rows(&self, range: Range<usize>, cx: &mut Context<Self>) -> Vec<AnyElement> {
        let colors = cx.colors().clone();
        range
            .map(|row| {
                let selected = self.selected == Some(row);
                h_flex()
                    .id(ElementId::Integer(row as u64))
                    .relative()
                    .h(u(self.row_height))
                    .px(u(12.0))
                    .border_b_1()
                    .border_color(colors.row_border)
                    .whitespace_nowrap()
                    .map(|this| {
                        if selected {
                            this.bg(colors.selection).child(
                                div()
                                    .absolute()
                                    .inset_0()
                                    .border_1()
                                    .border_color(colors.accent),
                            )
                        } else {
                            this.hover(|s| s.bg(colors.hover))
                        }
                    })
                    .children(self.columns.iter().enumerate().map(|(column, def)| {
                        let value = self.delegate.cell(row, column, cx);
                        column_cell(def).child(render_cell(value, def, &colors))
                    }))
                    .on_click(
                        cx.listener(move |this, event: &gpui::ClickEvent, window, cx| {
                            this.focus.focus(window, cx);
                            this.select(Some(row), cx);
                            if event.click_count() == 2 {
                                cx.emit(DataTableEvent::Confirmed(row));
                            }
                        }),
                    )
                    .into_any_element()
            })
            .collect()
    }

    fn render_header(&self, cx: &mut Context<Self>) -> impl IntoElement {
        let colors = cx.colors();
        h_flex()
            .flex_none()
            .h(u(sizes::TABLE_HEADER))
            .px(u(12.0))
            .bg(colors.subheader_background)
            .border_b_1()
            .border_color(colors.border_variant)
            .text_size(u(11.5))
            .text_color(colors.text_dim)
            .whitespace_nowrap()
            .children(self.columns.iter().enumerate().map(|(column, def)| {
                let resizable = matches!(def.width, ColumnWidth::Fixed(_));
                column_cell(def)
                    .relative()
                    .child(def.title.to_uppercase())
                    .when(resizable, |this| {
                        this.child(
                            div()
                                .id(SharedString::from(format!("resize-{column}")))
                                .absolute()
                                .top_0()
                                .bottom_0()
                                .right(u(-3.0))
                                .w(u(6.0))
                                .cursor_col_resize()
                                .on_mouse_down(
                                    MouseButton::Left,
                                    cx.listener(move |this, event, _, cx| {
                                        this.start_resize(column, event, cx)
                                    }),
                                ),
                        )
                    })
            }))
    }
}

/// A cell container sized by its column definition.
fn column_cell(def: &ColumnDef) -> gpui::Div {
    let cell = div().min_w_0().overflow_hidden().pr(u(8.0));
    let cell = match def.width {
        ColumnWidth::Fixed(width) => cell.flex_none().w(u(width)),
        ColumnWidth::Flex { weight, min } => {
            cell.flex_basis(u(0.0)).flex_grow(weight).min_w(u(min))
        }
    };
    match def.align {
        Align::Start => cell,
        Align::End => cell.flex().justify_end(),
    }
}

fn render_cell(value: CellValue, def: &ColumnDef, colors: &crate::Colors) -> AnyElement {
    let text = |content: SharedString| {
        div()
            .truncate()
            .when(def.mono, |this| {
                this.font_family(fonts::MONO).text_size(u(12.0))
            })
            .child(content)
    };
    match value {
        CellValue::Text(content) => text(content).into_any_element(),
        CellValue::Tinted { label, tone } => text(label)
            .text_color(crate::tone_color(tone, colors))
            .into_any_element(),
        CellValue::Status { label, tone } => StatusPill::new(label, tone).into_any_element(),
        CellValue::Usage { label, percent } => v_flex()
            .gap(u(3.0))
            .pr(u(12.0))
            .child(text(label))
            .child(ProgressBar::new(percent))
            .into_any_element(),
        CellValue::Empty => div()
            .font_family(fonts::MONO)
            .text_color(colors.text_faint)
            .child("—")
            .into_any_element(),
    }
}

impl Render for DataTable {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let row_count = self.delegate.row_count(cx);
        let resizing = self.resize.is_some();
        v_flex()
            .id("data-table")
            .key_context(CONTEXT)
            .track_focus(&self.focus)
            .size_full()
            .min_h_0()
            .text_size(u(sizes::UI_FONT))
            .text_color(cx.colors().text)
            .on_action(cx.listener(Self::select_next))
            .on_action(cx.listener(Self::select_previous))
            .on_action(cx.listener(Self::select_first))
            .on_action(cx.listener(Self::select_last))
            .on_action(cx.listener(Self::select_page_down))
            .on_action(cx.listener(Self::select_page_up))
            .on_action(cx.listener(Self::confirm))
            .child(self.render_header(cx))
            .child(
                uniform_list(
                    "rows",
                    row_count,
                    cx.processor(|this, range: Range<usize>, _, cx| this.render_rows(range, cx)),
                )
                .flex_1()
                .track_scroll(&self.scroll),
            )
            .when(resizing, |this| {
                this.cursor_col_resize()
                    .on_mouse_move(cx.listener(Self::drag_resize))
                    .on_mouse_up(
                        MouseButton::Left,
                        cx.listener(|this, _, _, cx| {
                            this.resize = None;
                            cx.notify();
                        }),
                    )
                    .on_mouse_up_out(
                        MouseButton::Left,
                        cx.listener(|this, _, _, cx| {
                            this.resize = None;
                            cx.notify();
                        }),
                    )
            })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use gpui::TestAppContext;

    struct Rows(usize);

    impl TableDelegate for Rows {
        fn columns(&self) -> Vec<ColumnDef> {
            vec![ColumnDef::new(
                "name",
                "Name",
                ColumnWidth::Flex {
                    weight: 1.0,
                    min: 100.0,
                },
            )]
        }

        fn row_count(&self, _: &App) -> usize {
            self.0
        }

        fn cell(&self, row: usize, _: usize, _: &App) -> CellValue {
            CellValue::Text(format!("row-{row}").into())
        }
    }

    #[gpui::test]
    fn keyboard_selection_is_clamped(cx: &mut TestAppContext) {
        let table = cx.new(|cx| DataTable::new(Rows(5_000), cx));
        let events = std::rc::Rc::new(std::cell::RefCell::new(Vec::new()));
        cx.update(|cx| {
            let events = events.clone();
            cx.subscribe(&table, move |_, event: &DataTableEvent, _| {
                events.borrow_mut().push(event.clone())
            })
            .detach();
        });
        table.update(cx, |table, cx| {
            table.move_selection(-1, cx);
            assert_eq!(table.selected(), Some(0));
            table.move_selection(PAGE as isize, cx);
            assert_eq!(table.selected(), Some(PAGE));
            table.select(Some(usize::MAX), cx);
            assert_eq!(table.selected(), Some(4_999));
            table.move_selection(1, cx);
            assert_eq!(table.selected(), Some(4_999));
        });
        assert_eq!(
            *events.borrow(),
            [
                DataTableEvent::SelectionChanged(Some(0)),
                DataTableEvent::SelectionChanged(Some(PAGE)),
                DataTableEvent::SelectionChanged(Some(4_999)),
            ]
        );
    }

    #[gpui::test]
    fn empty_table_has_no_selection(cx: &mut TestAppContext) {
        let table = cx.new(|cx| DataTable::new(Rows(0), cx));
        table.update(cx, |table, cx| {
            table.move_selection(1, cx);
            assert_eq!(table.selected(), None);
        });
    }
}
