//! `ViewKind::Events`: every event of a cluster (or namespace) as a table, with search,
//! Warning/Normal filters, grouping of repeats and pause. Selecting a row shows the object in
//! the details dock; Enter opens it.

use std::cell::RefCell;
use std::rc::Rc;
use std::sync::Arc;
use std::time::Duration;

use gpui::{
    App, AppContext as _, Context, Entity, FocusHandle, Focusable, FontWeight, IntoElement,
    KeyBinding, Render, SharedString, Subscription, Task, Window, actions, div, prelude::*,
};
use gpui_component::input::{Input, InputEvent, InputState};
use kubyl_core::{
    ActionRegistry, ActionSpec, ActiveContext, CellValue, ColumnDef, ColumnWidth, ResourceRef,
    TabView, Tone, ViewKind, ViewRequest,
};
use kubyl_resources::{ResourceSelection, Selected};
use kubyl_ui::{
    ActiveColors, Chip, DataTable, DataTableEvent, Icon, IconName, KeyHints, TableDelegate, fonts,
    h_flex, sizes, u, v_flex,
};

use super::feed::{EventsFeed, object_ref};
use super::model::{EventRow, Filter};
use super::ui;

actions!(
    events,
    [
        /// Focuses the search field of the Events view.
        FocusSearch,
        /// Pauses or resumes the stream on screen.
        TogglePause,
        /// Groups repeated events (`×N`) or lists each one.
        ToggleGrouping,
        /// Moves focus from the search field back to the table.
        FocusTable,
    ]
);

const CONTEXT: &str = "EventsView";
const SEARCH_CONTEXT: &str = "EventsSearch";

pub(crate) fn init(cx: &mut App) {
    cx.bind_keys([
        KeyBinding::new("escape", FocusTable, Some(SEARCH_CONTEXT)),
        KeyBinding::new("down", FocusTable, Some(SEARCH_CONTEXT)),
    ]);
    for (spec, keys) in [
        (
            ActionSpec::new("Events: Search", FocusSearch).hint("Search"),
            "/",
        ),
        (
            ActionSpec::new("Events: Pause or Resume", TogglePause).hint("Pause"),
            "p",
        ),
        (
            ActionSpec::new("Events: Group Repeated", ToggleGrouping).hint("Group"),
            "g",
        ),
    ] {
        ActionRegistry::register(cx, spec.bind(keys, Some(CONTEXT)));
    }
}

struct Rows {
    rows: Rc<RefCell<Vec<EventRow>>>,
    now: Rc<RefCell<jiff::Timestamp>>,
}

impl TableDelegate for Rows {
    fn columns(&self) -> Vec<ColumnDef> {
        vec![
            ColumnDef::new("last_seen", "Last seen", ColumnWidth::Fixed(84.0)).mono(),
            ColumnDef::new("type", "Type", ColumnWidth::Fixed(96.0)),
            ColumnDef::new("reason", "Reason", ColumnWidth::Fixed(160.0)),
            ColumnDef::new("object", "Object", ColumnWidth::Fixed(280.0)).mono(),
            ColumnDef::new(
                "message",
                "Message",
                ColumnWidth::Flex {
                    weight: 1.0,
                    min: 240.0,
                },
            ),
            ColumnDef::new("count", "Count", ColumnWidth::Fixed(64.0))
                .mono()
                .align_end(),
            ColumnDef::new("namespace", "Namespace", ColumnWidth::Fixed(130.0)).mono(),
            ColumnDef::new("source", "Source", ColumnWidth::Fixed(150.0)),
        ]
    }

    fn row_count(&self, _: &App) -> usize {
        self.rows.borrow().len()
    }

    fn cell(&self, row: usize, column: usize, _: &App) -> CellValue {
        let rows = self.rows.borrow();
        let Some(event) = rows.get(row) else {
            return CellValue::Empty;
        };
        let muted = |label: String| CellValue::Tinted {
            label: label.into(),
            tone: Tone::Neutral,
        };
        match column {
            0 => muted(ui::age(event, *self.now.borrow())),
            1 => CellValue::Status {
                label: if event.warning { "Warning" } else { "Normal" }.into(),
                tone: if event.warning {
                    Tone::Warning
                } else {
                    Tone::Muted
                },
            },
            2 => CellValue::Tinted {
                label: event.reason.clone(),
                tone: if event.warning {
                    Tone::Warning
                } else {
                    Tone::Neutral
                },
            },
            3 => CellValue::Text(event.object().into()),
            4 => CellValue::Text(event.message.clone()),
            5 => CellValue::Text(event.count.to_string().into()),
            6 => muted(event.namespace.as_deref().unwrap_or_default().to_string()),
            7 => muted(event.source.to_string()),
            _ => CellValue::Empty,
        }
    }
}

pub struct EventsView {
    /// The cluster; `namespace: None` follows the active namespace.
    target: ResourceRef,
    feed: Entity<EventsFeed>,
    table: Entity<DataTable>,
    rows: Rc<RefCell<Vec<EventRow>>>,
    now: Rc<RefCell<jiff::Timestamp>>,
    filter: Filter,
    search: Entity<InputState>,
    /// Namespace picked in the chip (`Some(None)` = all).
    picked: Option<Option<String>>,
    selected: Option<SharedString>,
    focus: FocusHandle,
    _ticker: Task<()>,
    _subscriptions: Vec<Subscription>,
}

impl EventsView {
    pub fn new(target: ResourceRef, window: &mut Window, cx: &mut Context<Self>) -> Self {
        let feed = cx.new(EventsFeed::new);
        let rows = Rc::new(RefCell::new(Vec::new()));
        let now = Rc::new(RefCell::new(jiff::Timestamp::now()));
        let table = cx.new(|cx| {
            DataTable::new(
                Rows {
                    rows: rows.clone(),
                    now: now.clone(),
                },
                cx,
            )
        });
        let search =
            cx.new(|cx| InputState::new(window, cx).placeholder("Search reason, object, message"));
        let subscriptions = vec![
            cx.observe(&feed, |this, _, cx| this.refresh(cx)),
            cx.observe_global::<ActiveContext>(|this, cx| this.follow(cx)),
            cx.subscribe_in(
                &table,
                window,
                |this, _, event: &DataTableEvent, window, cx| this.table_event(event, window, cx),
            ),
            cx.subscribe_in(
                &search,
                window,
                |this, input, event: &InputEvent, window, cx| match event {
                    InputEvent::Change => {
                        this.filter.search = input.read(cx).value().to_string();
                        this.refresh(cx);
                    }
                    InputEvent::PressEnter { .. } => {
                        let focus = this.table.read(cx).focus_handle(cx);
                        focus.focus(window, cx);
                    }
                    _ => {}
                },
            ),
        ];
        let ticker = cx.spawn(async move |this, cx| {
            loop {
                cx.background_executor().timer(Duration::from_secs(5)).await;
                if this.update(cx, |_, cx| cx.notify()).is_err() {
                    break;
                }
            }
        });
        let mut this = Self {
            target,
            feed,
            table,
            rows,
            now,
            filter: Filter::default(),
            search,
            picked: None,
            selected: None,
            focus: cx.focus_handle(),
            _ticker: ticker,
            _subscriptions: subscriptions,
        };
        this.follow(cx);
        this
    }

    /// The target's namespace, else the picked one, else the active namespace of this cluster.
    fn follow(&mut self, cx: &mut Context<Self>) {
        let namespace = match (&self.target.namespace, &self.picked) {
            (Some(ns), None) => Some(ns.clone()),
            (_, Some(picked)) => picked.clone(),
            (None, None) => {
                let active = ActiveContext::global(cx);
                if active.cluster.as_ref().map(|c| &c.id) == Some(&self.target.cluster) {
                    active.namespace.as_ref().map(|n| n.to_string())
                } else {
                    None
                }
            }
        };
        let cluster = self.target.cluster.clone();
        self.feed
            .update(cx, |feed, cx| feed.set_scope(Some(cluster), namespace, cx));
    }

    fn refresh(&mut self, cx: &mut Context<Self>) {
        let rows: Vec<EventRow> = self
            .feed
            .read(cx)
            .rows()
            .iter()
            .filter(|r| self.filter.matches(r))
            .cloned()
            .collect();
        let index = self
            .selected
            .as_ref()
            .and_then(|key| rows.iter().position(|r| &r.key == key));
        *self.rows.borrow_mut() = rows;
        *self.now.borrow_mut() = jiff::Timestamp::now();
        self.table.update(cx, |table, cx| {
            if index.is_some() {
                table.select(index, cx);
            }
            cx.notify();
        });
        cx.notify();
    }

    fn table_event(&mut self, event: &DataTableEvent, window: &mut Window, cx: &mut Context<Self>) {
        match event {
            DataTableEvent::SelectionChanged(index) => {
                let row = index.and_then(|i| self.rows.borrow().get(i).cloned());
                self.selected = row.as_ref().map(|r| r.key.clone());
                // The details dock follows the selection, like in resource lists.
                if let Some(row) = row
                    && let Some(target) = object_ref(&row, &self.target.cluster, cx)
                {
                    let caps = kubyl_kube::ConnectionManager::try_global(cx)
                        .map(|m| m.read(cx).caps(&self.target.cluster))
                        .unwrap_or_default();
                    ResourceSelection::set(
                        cx,
                        ResourceSelection {
                            items: vec![Selected {
                                target,
                                kind: row.kind.to_string(),
                                object: None,
                                store: None,
                            }],
                            caps,
                        },
                    );
                }
            }
            DataTableEvent::Confirmed(index) => {
                let row = self.rows.borrow().get(*index).cloned();
                if let Some(row) = row {
                    ui::open_object(&row, &self.feed, window, cx);
                }
            }
        }
    }

    fn set_type(&mut self, value: Option<bool>, cx: &mut Context<Self>) {
        self.filter.warnings = value;
        self.refresh(cx);
    }

    fn scope_label(&self, cx: &App) -> String {
        match self.feed.read(cx).namespace() {
            Some(ns) => format!("in {ns}"),
            None => "in all namespaces".into(),
        }
    }
}

impl Focusable for EventsView {
    /// The table, so arrow keys and `j`/`k` work right away.
    fn focus_handle(&self, cx: &App) -> FocusHandle {
        self.table.read(cx).focus_handle(cx)
    }
}

impl TabView for EventsView {
    fn tab_title(&self, _: &App) -> SharedString {
        "Events".into()
    }

    fn tab_icon(&self, _: &App) -> Option<SharedString> {
        Some(IconName::Bell.path())
    }

    fn view_request(&self, _: &App) -> Option<ViewRequest> {
        Some(ViewRequest::for_resource(
            ViewKind::Events,
            self.target.clone(),
        ))
    }
}

impl Render for EventsView {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let colors = cx.colors().clone();
        *self.now.borrow_mut() = jiff::Timestamp::now();
        let feed = self.feed.read(cx);
        let all: Arc<Vec<EventRow>> = feed.rows();
        let status = feed.status(cx);
        let folded = feed.is_folded();
        let shown = self.rows.borrow().len();
        let warnings = all.iter().filter(|r| r.warning).count();
        let search_focused = self.search.read(cx).focus_handle(cx).is_focused(window);
        let weak = cx.weak_entity();

        let crumb = h_flex()
            .flex_none()
            .gap(u(6.0))
            .text_color(colors.text_dim)
            .child(Icon::new(IconName::Bell).color(colors.accent))
            .child(
                div()
                    .text_color(colors.text)
                    .font_weight(FontWeight::MEDIUM)
                    .child("Events"),
            )
            .child("·")
            .child(format!("{shown} {}", self.scope_label(cx)))
            .when(warnings > 0, |this| {
                this.child(
                    div()
                        .text_color(colors.yellow)
                        .child(format!("· {warnings} warnings")),
                )
            });
        let search = div()
            .key_context(SEARCH_CONTEXT)
            .on_action(cx.listener(|this, _: &FocusTable, window, cx| {
                let focus = this.table.read(cx).focus_handle(cx);
                focus.focus(window, cx);
            }))
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
            .border_color(if search_focused {
                colors.accent
            } else {
                colors.border
            })
            .text_size(u(12.0))
            .child(Icon::new(IconName::Search).size(12.0))
            .child(
                div()
                    .flex_1()
                    .min_w_0()
                    .child(Input::new(&self.search).appearance(false)),
            );
        let on_type = {
            let weak = weak.clone();
            Rc::new(move |value: Option<bool>, _: &mut Window, cx: &mut App| {
                weak.update(cx, |this, cx| this.set_type(value, cx)).ok();
            })
        };
        let on_namespace = {
            let weak = weak.clone();
            Rc::new(move |ns: Option<String>, _: &mut Window, cx: &mut App| {
                weak.update(cx, |this, cx| {
                    this.picked = Some(ns);
                    this.follow(cx);
                })
                .ok();
            })
        };
        let feed_handle = self.feed.clone();
        let group_chip = div()
            .id("events-group")
            .cursor_pointer()
            .child(Chip::new("×N grouped").selected(folded))
            .on_click(move |_, _, cx| {
                feed_handle.update(cx, |feed, cx| feed.set_folded(!folded, cx))
            });
        let toolbar = h_flex()
            .flex_none()
            .h(u(sizes::TOOLBAR))
            .px(u(12.0))
            .gap(u(8.0))
            .border_b_1()
            .border_color(colors.border_variant)
            .child(crumb)
            .child(div().flex_1())
            .child(search)
            .child(ui::type_chips(&all, &self.filter, on_type, &colors))
            .child(ui::namespace_chip(&self.feed, on_namespace, cx))
            .child(group_chip)
            .child(ui::live_indicator(&status, &colors))
            .child(ui::pause_button(&self.feed, cx));
        let hints: Vec<(SharedString, SharedString)> = vec![
            ("enter".into(), "Open object".into()),
            ("/".into(), "Search".into()),
            ("p".into(), "Pause".into()),
            ("g".into(), "Group".into()),
        ];

        v_flex()
            .key_context(CONTEXT)
            .track_focus(&self.focus)
            .size_full()
            .text_color(colors.text)
            .on_action(cx.listener(|this, _: &FocusSearch, window, cx| {
                let focus = this.search.read(cx).focus_handle(cx);
                focus.focus(window, cx);
            }))
            .on_action(cx.listener(|this, _: &TogglePause, _, cx| {
                this.feed.update(cx, |feed, cx| {
                    let paused = feed.is_paused();
                    feed.set_paused(!paused, cx)
                })
            }))
            .on_action(cx.listener(|this, _: &ToggleGrouping, _, cx| {
                this.feed.update(cx, |feed, cx| {
                    let folded = feed.is_folded();
                    feed.set_folded(!folded, cx)
                })
            }))
            .child(toolbar)
            .child(div().flex_1().min_h_0().flex().child(self.table.clone()))
            .child(KeyHints::new(hints))
            .font_family(fonts::UI)
    }
}
