//! Rendering of [`ResourceListView`]: toolbar, header, virtualized rows and the key-hint bar.

use std::ops::Range;

use gpui::{
    AnyElement, Context, ElementId, Focusable as _, FontWeight, IntoElement, MouseButton,
    MouseDownEvent, MouseMoveEvent, SharedString, Window, div, prelude::*, uniform_list,
};
use gpui_component::button::{Button as MenuButton, ButtonVariants as _};
use gpui_component::input::Input;
use gpui_component::menu::{ContextMenuExt as _, DropdownMenu as _, PopupMenuItem};
use kubyl_core::{ActionRegistry, Align, CellValue, ColumnDef, ColumnWidth};
use kubyl_kube::ConnectionManager;
use kubyl_ui::{
    ActiveColors, Chip, Colors, Icon, IconName, KeyHints, ProgressBar, StatusPill, fonts, h_flex,
    sizes, tone_color, u, v_flex,
};

use super::{
    ExtendNext, ExtendPrevious, FILTER_CONTEXT, FocusFilter, FocusTable, OpenSelected, PAGE,
    Resize, ResourceListView, SelectAll, SelectFirst, SelectLast, SelectNext, SelectPageDown,
    SelectPageUp, SelectPrevious, SetFilter, ToggleMark, ToggleWide,
};

const ROW_HEIGHT: f32 = 32.0;
const MIN_COLUMN_WIDTH: f32 = 40.0;

pub(super) fn render(
    view: &mut ResourceListView,
    window: &mut Window,
    cx: &mut Context<ResourceListView>,
) -> impl IntoElement {
    let colors = cx.colors().clone();
    let columns = view.visible_columns();
    let hints = view.hints(cx);
    let toolbar = render_toolbar(view, window, cx);
    let header = render_header(view, &columns, cx);
    let empty = view.empty_message(cx);
    let resizing = view.resize.is_some();
    let focus = view.focus.clone();

    let body = match empty {
        Some(message) => div()
            .flex_1()
            .flex()
            .items_center()
            .justify_center()
            .text_color(colors.text_dim)
            .child(message)
            .into_any_element(),
        None => uniform_list(
            "rows",
            view.rows.len(),
            cx.processor(move |this, range: Range<usize>, _, cx| render_rows(this, range, cx)),
        )
        .flex_1()
        .track_scroll(&view.scroll)
        .into_any_element(),
    };

    let context_focus = focus.clone();
    let weak = cx.entity().downgrade();
    let table = v_flex()
        .id("resource-table")
        .key_context(view.key_context())
        .track_focus(&focus)
        .flex_1()
        .min_h_0()
        .on_action(cx.listener(|this, _: &SelectNext, _, cx| this.move_selection(1, false, cx)))
        .on_action(
            cx.listener(|this, _: &SelectPrevious, _, cx| this.move_selection(-1, false, cx)),
        )
        .on_action(cx.listener(|this, _: &ExtendNext, _, cx| this.move_selection(1, true, cx)))
        .on_action(cx.listener(|this, _: &ExtendPrevious, _, cx| this.move_selection(-1, true, cx)))
        .on_action(cx.listener(|this, _: &SelectFirst, _, cx| {
            this.marked.clear();
            this.select_index(Some(0), cx)
        }))
        .on_action(cx.listener(|this, _: &SelectLast, _, cx| {
            this.marked.clear();
            this.select_index(Some(usize::MAX), cx)
        }))
        .on_action(cx.listener(|this, _: &SelectPageDown, _, cx| {
            this.move_selection(PAGE as isize, false, cx)
        }))
        .on_action(cx.listener(|this, _: &SelectPageUp, _, cx| {
            this.move_selection(-(PAGE as isize), false, cx)
        }))
        .on_action(cx.listener(|this, _: &ToggleMark, _, cx| {
            if let Some(id) = this.selected.clone()
                && !this.marked.remove(&id)
            {
                this.marked.insert(id);
            }
            this.publish_selection(cx);
            cx.notify();
        }))
        .on_action(cx.listener(|this, _: &SelectAll, _, cx| {
            this.marked = this.rows.iter().map(|r| r.id()).collect();
            this.publish_selection(cx);
            cx.notify();
        }))
        .on_action(cx.listener(|this, _: &OpenSelected, window, cx| this.open_selected(window, cx)))
        .on_action(cx.listener(|this, _: &FocusFilter, window, cx| {
            let focus = this.filter_input.read(cx).focus_handle(cx);
            focus.focus(window, cx);
        }))
        .on_action(cx.listener(|this, _: &ToggleWide, _, cx| this.toggle_wide(cx)))
        .on_action(cx.listener(|this, action: &SetFilter, window, cx| {
            this.apply_filter(&action.query, window, cx)
        }))
        .child(header)
        .child(body)
        .when(resizing, |this| {
            this.cursor_col_resize()
                .on_mouse_move(cx.listener(drag_resize))
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
        .context_menu(move |menu, _, cx| {
            let mut menu = menu.action_context(context_focus.clone());
            let selection = kubyl_resources::ResourceSelection::global(cx).clone();
            if let Some(primary) = selection.primary() {
                let caps = selection.caps.clone();
                for spec in ActionRegistry::global(cx).all() {
                    let applies = spec
                        .context
                        .as_deref()
                        .is_some_and(|c| c.starts_with(super::CONTEXT))
                        && spec.is_available(&primary.target, &caps)
                        && spec.hint.is_some();
                    if applies {
                        let label = spec.hint.clone().unwrap_or_else(|| spec.name.clone());
                        menu = menu.menu(label, spec.action());
                    }
                }
                menu = menu.separator();
                menu = menu.menu("Copy Name", Box::new(crate::actions::CopyName));
                menu = menu.menu("Copy YAML", Box::new(crate::actions::CopyYaml));
                menu = menu.menu("Open in New Tab", Box::new(crate::actions::OpenInNewTab));
                menu = menu.menu("Open in Split", Box::new(crate::actions::OpenInSplit));
            }
            let weak = weak.clone();
            menu.separator()
                .item(
                    PopupMenuItem::new("Add Namespace to Favorites").on_click(move |_, _, cx| {
                        weak.update(cx, |view, cx| view.favorite_namespace(cx)).ok();
                    }),
                )
        });

    v_flex()
        .size_full()
        .bg(colors.background)
        .child(toolbar)
        .child(table)
        .child(KeyHints::new(hints))
}

fn render_toolbar(
    view: &mut ResourceListView,
    window: &mut Window,
    cx: &mut Context<ResourceListView>,
) -> impl IntoElement + use<> {
    let colors = cx.colors().clone();
    let total = view.rows.len();
    let (live_label, live_tone) = view.status_summary(cx);
    let live_color = tone_color(live_tone, &colors);
    let filter_focused = view
        .filter_input
        .read(cx)
        .focus_handle(cx)
        .is_focused(window);
    let weak = cx.entity().downgrade();

    let crumb = h_flex()
        .flex_none()
        .gap(u(6.0))
        .text_color(colors.text_dim)
        .child(Icon::new(view.icon()).color(colors.accent))
        .child(
            div()
                .text_color(colors.text)
                .font_weight(FontWeight::MEDIUM)
                .child(view.kind_label()),
        )
        .child("·")
        .child(format!("{total} {}", view.namespace_label()))
        .when(view.failing > 0, |this| {
            this.child(
                div()
                    .text_color(colors.red)
                    .child(format!("· {} failing", view.failing)),
            )
        });

    let filter = div()
        .key_context(FILTER_CONTEXT)
        .on_action(cx.listener(|this, _: &FocusTable, window, cx| this.focus.focus(window, cx)))
        .flex_none()
        .w(u(300.0))
        .h(u(sizes::CONTROL))
        .px(u(8.0))
        .flex()
        .items_center()
        .gap(u(7.0))
        .rounded(u(5.0))
        .bg(colors.input_background)
        .border_1()
        .border_color(if view.filter_error.is_some() {
            colors.red
        } else if filter_focused {
            colors.accent
        } else {
            colors.border
        })
        .font_family(fonts::MONO)
        .text_size(u(12.0))
        .child(Icon::new(IconName::Funnel).size(12.0))
        .child(
            div()
                .flex_1()
                .min_w_0()
                .child(Input::new(&view.filter_input).appearance(false)),
        );

    let mut chips = h_flex().flex_none().gap(u(4.0));
    if view.namespaced && view.cluster().is_some() {
        for namespace in view.namespaces.clone() {
            let weak = weak.clone();
            let ns = namespace.clone();
            chips = chips.child(
                div()
                    .id(SharedString::from(format!("ns-chip-{namespace}")))
                    .cursor_pointer()
                    .on_click(move |_, window, cx| {
                        let ns = ns.clone();
                        weak.update(cx, |this, cx| {
                            let rest: Vec<String> = this
                                .namespaces
                                .iter()
                                .filter(|n| **n != ns)
                                .cloned()
                                .collect();
                            this.set_namespaces(rest, window, cx);
                        })
                        .ok();
                    })
                    .child(Chip::new(namespace).selected(true).removable()),
            );
        }
        let available = view
            .cluster()
            .and_then(|c| ConnectionManager::try_global(cx).map(|m| m.read(cx).namespaces(c).names))
            .unwrap_or_default();
        let selected = view.namespaces.clone();
        let weak = weak.clone();
        chips = chips.child(
            MenuButton::new("add-namespace")
                .ghost()
                .compact()
                .p_0()
                .child(Chip::new(if selected.is_empty() {
                    "all namespaces"
                } else {
                    "+ namespace"
                }))
                .dropdown_menu(move |menu, _, _| {
                    let mut menu = menu.max_h(gpui::px(420.0)).scrollable(true);
                    let all = weak.clone();
                    menu = menu.item(
                        PopupMenuItem::new("All namespaces")
                            .checked(selected.is_empty())
                            .on_click(move |_, window, cx| {
                                all.update(cx, |this, cx| {
                                    this.set_namespaces(Vec::new(), window, cx)
                                })
                                .ok();
                            }),
                    );
                    menu = menu.separator();
                    for namespace in &available {
                        let weak = weak.clone();
                        let ns = namespace.clone();
                        menu = menu.item(
                            PopupMenuItem::new(namespace.clone())
                                .checked(selected.contains(namespace))
                                .on_click(move |event, window, cx| {
                                    let ns = ns.clone();
                                    let add =
                                        event.modifiers().secondary() || event.modifiers().shift;
                                    weak.update(cx, |this, cx| {
                                        let mut next = if add {
                                            this.namespaces.clone()
                                        } else {
                                            Vec::new()
                                        };
                                        if let Some(ix) = next.iter().position(|n| *n == ns) {
                                            next.remove(ix);
                                        } else {
                                            next.push(ns);
                                        }
                                        next.sort();
                                        this.set_namespaces(next, window, cx);
                                    })
                                    .ok();
                                }),
                        );
                    }
                    if available.is_empty() {
                        menu = menu.label("No namespaces listed (forbidden or not connected)");
                    } else {
                        menu = menu.separator().label("⌘/⇧-click to select several");
                    }
                    menu
                }),
        );
    }

    let column_ids: Vec<(SharedString, SharedString, bool)> = view
        .columns
        .iter()
        .filter(|c| c.id.as_ref() != "name" && (view.prefs.wide || !c.wide))
        .map(|c| {
            let visible = !view.prefs.hidden.iter().any(|h| h == c.id.as_ref());
            (c.id.clone(), c.title.clone(), visible)
        })
        .collect();
    let wide = view.prefs.wide;
    let columns_weak = weak.clone();
    let columns_button = MenuButton::new("columns")
        .ghost()
        .compact()
        .child(Icon::new(IconName::SlidersVertical).size(13.0))
        .dropdown_menu(move |menu, _, _| {
            let mut menu = menu.label("Columns");
            for (id, title, visible) in &column_ids {
                let weak = columns_weak.clone();
                let id = id.clone();
                menu = menu.item(
                    PopupMenuItem::new(title.clone())
                        .checked(*visible)
                        .on_click(move |_, _, cx| {
                            weak.update(cx, |this, cx| this.toggle_column(&id, cx)).ok();
                        }),
                );
            }
            let weak = columns_weak.clone();
            menu.separator()
                .item(PopupMenuItem::new("Wide (-o wide)").checked(wide).on_click(
                    move |_, _, cx| {
                        weak.update(cx, |this, cx| this.toggle_wide(cx)).ok();
                    },
                ))
        });

    h_flex()
        .flex_none()
        .h(u(sizes::TOOLBAR))
        .px(u(12.0))
        .gap(u(8.0))
        .border_b_1()
        .border_color(colors.border_variant)
        .child(crumb)
        .child(div().flex_1())
        .child(filter)
        .child(chips)
        .child(columns_button)
        .child(Chip::new(live_label).dot(live_color).text_color(live_color))
}

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
        Align::End => cell.flex().justify_end().pr(u(18.0)),
    }
}

fn render_header(
    view: &ResourceListView,
    columns: &[ColumnDef],
    cx: &mut Context<ResourceListView>,
) -> impl IntoElement + use<> {
    let colors = cx.colors().clone();
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
        .overflow_hidden()
        .children(columns.iter().map(|def| {
            let id = def.id.clone();
            let sorted = match &view.sort {
                Some((column, ascending)) if *column == def.id => Some(*ascending),
                None if def.id.as_ref() == "name" => Some(true),
                _ => None,
            };
            let resizable = matches!(def.width, ColumnWidth::Fixed(_));
            let resize_id = id.clone();
            column_cell(def)
                .relative()
                .child(
                    h_flex()
                        .id(ElementId::Name(format!("sort-{id}").into()))
                        .gap(u(3.0))
                        .cursor_pointer()
                        .when(sorted.is_some(), |this| this.text_color(colors.text_muted))
                        .child(def.title.to_uppercase())
                        .when_some(sorted, |this, ascending| {
                            this.child(
                                Icon::new(if ascending {
                                    IconName::ChevronDown
                                } else {
                                    IconName::ArrowUp
                                })
                                .size(10.0)
                                .color(colors.text_dim),
                            )
                        })
                        .on_click(cx.listener(move |this, _, window, cx| {
                            this.set_sort(id.clone(), window, cx)
                        })),
                )
                .when(resizable, |this| {
                    this.child(
                        div()
                            .id(ElementId::Name(format!("resize-{resize_id}").into()))
                            .absolute()
                            .top_0()
                            .bottom_0()
                            .right(u(-3.0))
                            .w(u(6.0))
                            .cursor_col_resize()
                            .on_mouse_down(
                                MouseButton::Left,
                                cx.listener(move |this, event: &MouseDownEvent, _, cx| {
                                    let width = this
                                        .visible_columns()
                                        .iter()
                                        .find(|c| c.id == resize_id)
                                        .and_then(|c| match c.width {
                                            ColumnWidth::Fixed(w) => Some(w),
                                            ColumnWidth::Flex { .. } => None,
                                        });
                                    if let Some(width) = width {
                                        this.resize = Some(Resize {
                                            column: resize_id.clone(),
                                            start_x: event.position.x,
                                            start_width: width,
                                        });
                                        cx.stop_propagation();
                                    }
                                }),
                            ),
                    )
                })
        }))
}

fn drag_resize(
    this: &mut ResourceListView,
    event: &MouseMoveEvent,
    window: &mut Window,
    cx: &mut Context<ResourceListView>,
) {
    let Some(resize) = &this.resize else {
        return;
    };
    let scale = f32::from(window.rem_size()) / 16.0;
    let delta = f32::from(event.position.x - resize.start_x) / scale;
    let width = (resize.start_width + delta).max(MIN_COLUMN_WIDTH);
    this.widths.insert(resize.column.clone(), width);
    cx.notify();
}

fn render_rows(
    view: &mut ResourceListView,
    range: Range<usize>,
    cx: &mut Context<ResourceListView>,
) -> Vec<AnyElement> {
    let colors = cx.colors().clone();
    let columns = view.visible_columns();
    range
        .filter_map(|index| {
            let row = view.rows.get(index)?.clone();
            let selected = view.selected_index == Some(index);
            let marked = view.is_marked(&row);
            let hover = colors.hover;
            Some(
                h_flex()
                    .id(ElementId::Integer(index as u64))
                    .relative()
                    .w_full()
                    .h(u(ROW_HEIGHT))
                    .px(u(12.0))
                    .border_b_1()
                    .border_color(colors.row_border)
                    .whitespace_nowrap()
                    .overflow_hidden()
                    .text_size(u(sizes::UI_FONT))
                    .text_color(colors.text)
                    .when(marked, |this| this.bg(colors.selection))
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
                            this.hover(move |s| s.bg(hover))
                        }
                    })
                    .when(marked && !selected, |this| {
                        this.child(
                            div()
                                .absolute()
                                .left_0()
                                .top_0()
                                .bottom_0()
                                .w(u(2.0))
                                .bg(colors.accent),
                        )
                    })
                    .children(columns.iter().map(|def| {
                        let value = view.cell(&row, def, cx);
                        column_cell(def).child(render_cell(value, def, &colors))
                    }))
                    .on_mouse_down(
                        MouseButton::Right,
                        cx.listener(move |this, _, window, cx| {
                            let already = this.selected_index == Some(index)
                                || this.rows.get(index).is_some_and(|r| this.is_marked(r));
                            if !already {
                                this.click_row(index, gpui::Modifiers::default(), window, cx);
                            }
                        }),
                    )
                    .on_click(
                        cx.listener(move |this, event: &gpui::ClickEvent, window, cx| {
                            this.click_row(index, event.modifiers(), window, cx);
                            if event.click_count() == 2 {
                                this.open_selected(window, cx);
                            }
                        }),
                    )
                    .into_any_element(),
            )
        })
        .collect()
}

fn render_cell(value: CellValue, def: &ColumnDef, colors: &Colors) -> AnyElement {
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
            .text_color(tone_color(tone, colors))
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
            .text_size(u(12.0))
            .text_color(colors.text_faint)
            .child("—")
            .into_any_element(),
    }
}
