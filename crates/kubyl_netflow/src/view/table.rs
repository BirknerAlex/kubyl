//! The flow table (board 18): newest first, a row per flow (or aggregated record), the details
//! pane beside it.

use std::ops::Range;

use gpui::{
    AnyElement, ClickEvent, Context, FontWeight, IntoElement, SharedString, Window, div,
    prelude::*, uniform_list,
};
use kubyl_core::{ColumnDef, ColumnWidth};
use kubyl_ui::{ActiveColors, Colors, Icon, IconName, fonts, h_flex, sizes, u, v_flex};

use super::{NetworkFlowsView, details, widgets};
use crate::model::{Endpoint, EndpointKind, Flow, Verdict};
use crate::service::{FlowService, FlowState};

const ROW_HEIGHT: f32 = 28.0;

/// The table's columns; `narrow` (a split pane) drops the direction and tightens the rest.
fn columns(bytes: bool, narrow: bool) -> Vec<ColumnDef> {
    let flex = |weight: f32, min: f32| ColumnWidth::Flex { weight, min };
    let m = |wide: f32, tight: f32| if narrow { tight } else { wide };
    // Room for an aggregate's interval (`14:02:45–03:00`).
    let mut columns = vec![ColumnDef::new(
        "time",
        "Time",
        ColumnWidth::Fixed(m(112.0, 108.0)),
    )];
    if !narrow {
        columns.push(ColumnDef::new("dir", "Dir", ColumnWidth::Fixed(36.0)));
    }
    columns.extend([
        ColumnDef::new("source", "Source", flex(1.3, m(130.0, 84.0))),
        ColumnDef::new("destination", "Destination", flex(1.3, m(130.0, 84.0))),
        ColumnDef::new(
            "protocol",
            if narrow { "Port" } else { "Protocol · port" },
            flex(1.05, m(100.0, 58.0)),
        ),
        ColumnDef::new("verdict", "Verdict", ColumnWidth::Fixed(m(92.0, 84.0))),
        ColumnDef::new("policy", "Policy", flex(1.0, m(110.0, 80.0))),
    ]);
    if bytes {
        columns.push(ColumnDef::new(
            "bytes",
            "Bytes",
            ColumnWidth::Fixed(m(70.0, 64.0)),
        ));
    }
    columns
}

fn cell(def: &ColumnDef) -> gpui::Div {
    let cell = div().min_w_0().overflow_hidden().pr(u(10.0));
    match def.width {
        ColumnWidth::Fixed(width) => cell.flex_none().w(u(width)),
        ColumnWidth::Flex { weight, min } => {
            cell.flex_basis(u(0.0)).flex_grow(weight).min_w(u(min))
        }
    }
}

fn header(columns: &[ColumnDef], colors: &Colors) -> AnyElement {
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
            let title = def.title.to_uppercase();
            cell(def)
                .when(def.id.as_ref() == "bytes", |c| c.flex().justify_end())
                .child(div().truncate().child(title))
        }))
        .into_any_element()
}

/// `payments/checkout-api-…`, `🌐 api.bank.example.com`, `🖥 ip-10-0-12-41`.
/// `narrow`: the namespace gives way before the name (else the name is cut first).
pub(super) fn endpoint_cell(endpoint: &Endpoint, narrow: bool, colors: &Colors) -> AnyElement {
    let icon = match endpoint.kind {
        EndpointKind::World => Some(IconName::Globe),
        EndpointKind::Host | EndpointKind::RemoteNode | EndpointKind::KubeApiServer => {
            Some(IconName::Server)
        }
        _ => None,
    };
    let (namespace, name) = match (&endpoint.namespace, endpoint.kind) {
        (Some(ns), EndpointKind::Service) => (
            Some(ns.to_string()),
            endpoint.service.as_deref().unwrap_or("?").to_string(),
        ),
        (Some(ns), _) => (
            Some(ns.to_string()),
            endpoint
                .pod
                .as_deref()
                .or(endpoint.workload.as_ref().map(|w| w.name.as_ref()))
                .unwrap_or("?")
                .to_string(),
        ),
        (None, _) => (None, endpoint.label()),
    };
    h_flex()
        .min_w_0()
        .gap(u(5.0))
        .font_family(fonts::MONO)
        .text_size(u(11.5))
        .when_some(icon, |this, icon| {
            this.child(Icon::new(icon).size(12.0).color(colors.text_dim))
        })
        .child(
            div()
                .min_w_0()
                .truncate()
                .when_some(namespace, |this, ns| {
                    let mut namespace = div().text_color(colors.text_dim).child(format!("{ns}/"));
                    if narrow {
                        namespace = namespace.min_w(u(24.0)).truncate();
                        namespace.style().flex_shrink = Some(8.0);
                    } else {
                        namespace = namespace.flex_none();
                    }
                    this.child(namespace)
                })
                .flex()
                .child(div().min_w_0().truncate().child(name)),
        )
        .into_any_element()
}

fn render_cell(
    flow: &Flow,
    column: &str,
    today: jiff::civil::Date,
    narrow: bool,
    colors: &Colors,
) -> AnyElement {
    match column {
        "time" => div()
            .truncate()
            .font_family(fonts::MONO)
            .text_size(u(11.5))
            .text_color(colors.text_muted)
            .child(match flow.start {
                // Aggregated records (Whisker, NetObserv) cover an interval.
                Some(start) if start < flow.time => widgets::interval(start, flow.time, today),
                _ => widgets::clock(flow.time, today),
            })
            .into_any_element(),
        "dir" => div()
            .text_size(u(11.5))
            .text_color(colors.text_dim)
            .child(flow.direction.label())
            .into_any_element(),
        "source" => endpoint_cell(&flow.source, narrow, colors),
        "destination" => endpoint_cell(&flow.destination, narrow, colors),
        "protocol" => div()
            .truncate()
            .text_size(u(12.0))
            .when(flow.l7.is_some(), |this| {
                this.font_family(fonts::MONO).text_size(u(11.5))
            })
            .child(flow.protocol_label())
            .into_any_element(),
        "verdict" => widgets::verdict_pill(flow.verdict, colors),
        "policy" => widgets::policy_cell(&flow.policies.summary(flow.verdict), colors),
        "bytes" => div()
            .flex()
            .justify_end()
            .font_family(fonts::MONO)
            .text_size(u(11.5))
            .text_color(colors.text_dim)
            .child(flow.bytes.map(widgets::bytes).unwrap_or_else(|| "—".into()))
            .into_any_element(),
        _ => div().into_any_element(),
    }
}

impl NetworkFlowsView {
    fn render_rows(
        &mut self,
        range: Range<usize>,
        bytes: bool,
        cx: &mut Context<Self>,
    ) -> Vec<AnyElement> {
        let colors = cx.colors().clone();
        let narrow = self.narrow;
        let columns = columns(bytes, narrow);
        let today = jiff::Timestamp::now()
            .to_zoned(jiff::tz::TimeZone::system())
            .date();
        let Some(service) = FlowService::global(cx) else {
            return Vec::new();
        };
        let flows: Vec<(usize, u64, Option<std::sync::Arc<Flow>>)> = {
            let service = service.read(cx);
            let stream = service.stream(&self.cluster, &self.pushed);
            range
                .filter_map(|index| {
                    let seq = *self.rows.shown.get(index)?;
                    Some((index, seq, stream.and_then(|s| s.buffer.get(seq).cloned())))
                })
                .collect()
        };
        flows
            .into_iter()
            .map(|(index, seq, flow)| {
                let selected = self.selected == Some(seq);
                let row = h_flex()
                    .id(("flow-row", index))
                    .relative()
                    .w_full()
                    .h(u(ROW_HEIGHT))
                    .px(u(12.0))
                    .border_b_1()
                    .border_color(colors.row_border)
                    .whitespace_nowrap()
                    .overflow_hidden()
                    .text_size(u(sizes::UI_FONT))
                    .text_color(colors.text);
                let Some(flow) = flow else {
                    return row.into_any_element();
                };
                let tint = match flow.verdict {
                    Verdict::Dropped => Some(colors.error_row_background),
                    Verdict::NoReply => Some(colors.yellow.opacity(0.07)),
                    _ => None,
                };
                let hover = colors.hover;
                row.map(|this| {
                    if selected {
                        this.bg(colors.selection).child(
                            div()
                                .absolute()
                                .inset_0()
                                .border_1()
                                .border_color(colors.accent),
                        )
                    } else {
                        let this = match tint {
                            Some(tint) => this.bg(tint),
                            None => this,
                        };
                        this.hover(move |s| s.bg(hover))
                    }
                })
                .children(columns.iter().map(|def| {
                    cell(def).child(render_cell(&flow, def.id.as_ref(), today, narrow, &colors))
                }))
                .on_click(cx.listener(move |this, event: &ClickEvent, window, cx| {
                    this.focus.focus(window, cx);
                    this.select_seq(Some(seq), cx);
                    if event.click_count() == 2 {
                        this.set_details_open(true, cx);
                    }
                }))
                .into_any_element()
            })
            .collect()
    }
}

/// The Flows tab below the filter bar.
pub(super) fn render(
    view: &mut NetworkFlowsView,
    state: &FlowState,
    window: &mut Window,
    cx: &mut Context<NetworkFlowsView>,
) -> AnyElement {
    let colors = cx.colors().clone();
    let capabilities = match state {
        FlowState::Ready { capabilities, .. } => capabilities.clone(),
        _ => Default::default(),
    };
    // Back at the top: what waited joins.
    if !view.rows.pending.is_empty() && view.following() {
        view.rows.resume();
    }
    let status = super::stream_status(view, cx);
    let caught_up = status.as_ref().is_some_and(|(_, _, c)| *c);
    let notice = no_single_flows(state, &capabilities, &colors);
    let body = if view.rows.shown.is_empty() {
        let message: SharedString = if view.scanning {
            "Filtering…".into()
        } else if !capabilities.single_flows {
            "".into()
        } else if !caught_up && view.rows.pending.is_empty() {
            "Loading flows…".into()
        } else if !view.rows.pending.is_empty() {
            "New flows arrived: they show when you scroll to the top or resume.".into()
        } else if view.effective_filter().is_empty() {
            "No flows yet.".into()
        } else {
            "No flows match the filter.".into()
        };
        widgets::empty(message, &colors)
    } else {
        let columns = columns(capabilities.bytes, view.narrow);
        let bytes = capabilities.bytes;
        let waiting = view.rows.pending.len();
        let banner = (waiting > 0 && !view.paused).then(|| {
            let weak = cx.entity().downgrade();
            h_flex()
                .id("flows-new-above")
                .flex_none()
                .justify_center()
                .h(u(24.0))
                .gap(u(6.0))
                .bg(colors.chip_selected_background)
                .text_color(colors.chip_selected_text)
                .text_size(u(12.0))
                .cursor_pointer()
                .child(
                    Icon::new(IconName::ArrowUp)
                        .size(12.0)
                        .color(colors.chip_selected_text),
                )
                .child(format!(
                    "{} · show the newest",
                    widgets::plural(waiting, "new flow", "new flows")
                ))
                .on_click(move |_, _, cx| {
                    weak.update(cx, |this, cx| this.show_newest(cx)).ok();
                })
        });
        v_flex()
            .flex_1()
            .min_h_0()
            .child(header(&columns, &colors))
            .children(banner)
            .child(
                uniform_list(
                    "flow-rows",
                    view.rows.shown.len(),
                    cx.processor(move |this, range: Range<usize>, _, cx| {
                        this.render_rows(range, bytes, cx)
                    }),
                )
                .flex_1()
                .track_scroll(&view.scroll),
            )
            .into_any_element()
    };
    let details = (view.details_open)
        .then(|| view.selected_flow(cx))
        .flatten()
        .map(|flow| details::render(view, &flow, state, window, cx));
    v_flex()
        .size_full()
        .children(notice)
        .child(
            div()
                .key_context(super::LIST_CONTEXT)
                .track_focus(&view.focus)
                .flex()
                .flex_1()
                .min_h_0()
                .min_w_0()
                .child(v_flex().flex_1().min_w_0().h_full().child(body))
                .children(details),
        )
        .into_any_element()
}

/// Why a backend's Flows tab stays empty (NetObserv without Loki), above the table and the
/// topology.
pub(super) fn no_single_flows(
    state: &FlowState,
    capabilities: &crate::provider::Capabilities,
    colors: &Colors,
) -> Option<AnyElement> {
    (!capabilities.single_flows).then(|| {
        let reason = match state {
            FlowState::Ready { status, .. } => status.notes.join(" "),
            _ => String::new(),
        };
        h_flex()
            .flex_none()
            .items_start()
            .gap(u(10.0))
            .px(u(14.0))
            .py(u(10.0))
            .border_b_1()
            .border_color(colors.border_variant)
            .bg(colors.yellow.opacity(0.06))
            .text_size(u(12.5))
            .child(Icon::new(IconName::Info).size(14.0).color(colors.yellow))
            .child(
                v_flex()
                    .flex_1()
                    .min_w_0()
                    .gap(u(2.0))
                    .child(div().font_weight(FontWeight::SEMIBOLD).child("No single flows here"))
                    .child(div().text_color(colors.text_muted).child(
                        match reason.strip_prefix("No single flows: ").unwrap_or(&reason) {
                            "" => "This backend doesn't keep single flows Kubyl can read. The topology comes from its metrics.".to_string(),
                            reason => widgets::capitalize(reason),
                        },
                    )),
            )
            .into_any_element()
    })
}
