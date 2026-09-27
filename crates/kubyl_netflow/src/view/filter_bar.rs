//! The filter bar (input, completion, scope, term chips, verdict chips) and the header's backend
//! indicator, live state and menus.

use gpui::{
    AnyElement, App, Context, Focusable as _, IntoElement, SharedString, Window, div, prelude::*,
    px,
};
use gpui_component::button::{Button as MenuButton, ButtonVariants as _};
use gpui_component::input::Input;
use gpui_component::menu::{DropdownMenu as _, PopupMenuItem};
use kubyl_ui::{ActiveColors, Colors, Icon, IconName, fonts, h_flex, u, v_flex};

use super::{NetworkFlowsView, verdict_counts, widgets};
use crate::filter::{Field, SeenValues as _};
use crate::model::Verdict;
use crate::provider::BackendKind;
use crate::service::{FlowService, FlowState, StreamStatus};

pub(super) fn render(
    view: &mut NetworkFlowsView,
    state: &FlowState,
    window: &mut Window,
    cx: &mut Context<NetworkFlowsView>,
) -> AnyElement {
    let colors = cx.colors().clone();
    let _ = window;
    let error = view.parse_error.clone();
    let input = h_flex()
        .flex_1()
        .min_w_0()
        .h(u(28.0))
        .pl(u(8.0))
        .gap(u(6.0))
        .rounded(u(5.0))
        .bg(colors.input_background)
        .border_1()
        .border_color(if error.is_some() {
            colors.red
        } else {
            colors.border
        })
        .child(
            Icon::new(IconName::Funnel)
                .size(12.0)
                .color(colors.text_dim),
        )
        .child(
            div()
                .flex_1()
                .min_w_0()
                .font_family(fonts::MONO)
                .child(Input::new(&view.input).appearance(false).text_size(u(12.5))),
        );
    let top = h_flex()
        .gap(u(6.0))
        .child(input)
        .child(scope_menu(view, &colors, cx));
    let mut chips: Vec<AnyElement> = Vec::new();
    if let Some(error) = &error {
        chips.push(
            h_flex()
                .gap(u(5.0))
                .text_size(u(11.5))
                .text_color(colors.red)
                .child(Icon::new(IconName::CircleX).size(11.0).color(colors.red))
                .child(error.message.clone())
                .into_any_element(),
        );
    } else {
        chips.extend(term_chips(view, state, &colors, cx));
    }
    let matched = view.rows.shown.len() + view.rows.pending.len();
    let shown_count = if view.user_filter.is_empty() {
        None
    } else {
        Some(format!(
            "{} of {}",
            widgets::count(matched),
            widgets::plural(buffered(view, cx), "flow", "flows")
        ))
    };
    let bottom = h_flex()
        .gap(u(5.0))
        .min_h(u(20.0))
        .overflow_hidden()
        .children(chips)
        .when_some(shown_count, |this, text| {
            this.child(
                div()
                    .flex_none()
                    .ml(u(4.0))
                    .text_size(u(11.5))
                    .text_color(colors.text_dim)
                    .child(text),
            )
        })
        .child(div().flex_1())
        .children(verdict_chips(view, state, &colors, cx));
    let suggestions = (!view.suggestions.is_empty()
        && view.input.read(cx).focus_handle(cx).is_focused(window))
    .then(|| suggestions(view, &colors, cx));
    v_flex()
        .relative()
        .flex_none()
        .px(u(12.0))
        .py(u(7.0))
        .gap(u(6.0))
        .border_b_1()
        .border_color(colors.border_variant)
        .child(top)
        .child(bottom)
        .children(suggestions)
        .into_any_element()
}

/// Flows in the buffer (for "N of M").
fn buffered(view: &NetworkFlowsView, cx: &App) -> usize {
    FlowService::global(cx)
        .and_then(|s| {
            s.read(cx)
                .stream(&view.cluster, &view.pushed)
                .map(|st| st.buffer.len())
        })
        .unwrap_or(0)
}

fn term_chips(
    view: &NetworkFlowsView,
    state: &FlowState,
    colors: &Colors,
    cx: &mut Context<NetworkFlowsView>,
) -> Vec<AnyElement> {
    let backend = match state {
        FlowState::Ready { kind, .. } => Some(*kind),
        _ => None,
    };
    let mut out = Vec::new();
    let terms = view.user_filter.terms.clone();
    for (i, term) in terms.iter().enumerate() {
        let server = view.pushed.terms.contains(term);
        let tooltip: SharedString = match (server, backend) {
            (true, Some(kind)) => {
                format!("{} applies this term itself (server-side)", kind.label()).into()
            }
            _ => "Kubyl applies this term to the flows it holds (client-side)".into(),
        };
        let weak = cx.entity().downgrade();
        let text = term.to_string();
        out.push(
            h_flex()
                .id(("flow-term", i))
                .flex_none()
                .h(u(20.0))
                .px(u(7.0))
                .gap(u(5.0))
                .rounded(u(4.0))
                .bg(colors.chip_background)
                .font_family(fonts::MONO)
                .text_size(u(11.0))
                .text_color(colors.text_muted)
                .child(if server {
                    Icon::new(IconName::Zap).size(10.0).color(colors.accent)
                } else {
                    Icon::new(IconName::Funnel)
                        .size(10.0)
                        .color(colors.text_dim)
                })
                .child(text)
                .child(
                    div()
                        .id(("flow-term-remove", i))
                        .cursor_pointer()
                        .child(Icon::new(IconName::X).size(10.0).color(colors.text_dim))
                        .on_click(move |_, _, cx| {
                            cx.stop_propagation();
                            weak.update(cx, |this, cx| {
                                let mut filter = this.user_filter.clone();
                                if i < filter.terms.len() {
                                    filter.terms.remove(i);
                                }
                                this.set_query(filter.canonical(), cx);
                            })
                            .ok();
                        }),
                )
                .tooltip(move |window, cx| {
                    gpui_component::tooltip::Tooltip::new(tooltip.clone()).build(window, cx)
                })
                .into_any_element(),
        );
    }
    for (i, word) in view.user_filter.words.iter().enumerate() {
        out.push(
            h_flex()
                .id(("flow-word", i))
                .flex_none()
                .h(u(20.0))
                .px(u(7.0))
                .gap(u(5.0))
                .rounded(u(4.0))
                .bg(colors.chip_background)
                .text_size(u(11.0))
                .text_color(colors.text_muted)
                .child(
                    Icon::new(IconName::Search)
                        .size(10.0)
                        .color(colors.text_dim),
                )
                .child(format!("\u{201c}{word}\u{201d}"))
                .tooltip(|window, cx| {
                    gpui_component::tooltip::Tooltip::new(
                        "Free text: matched against every field in Kubyl",
                    )
                    .build(window, cx)
                })
                .into_any_element(),
        );
    }
    out
}

fn verdict_chips(
    view: &NetworkFlowsView,
    state: &FlowState,
    colors: &Colors,
    cx: &mut Context<NetworkFlowsView>,
) -> Vec<AnyElement> {
    if !matches!(state, FlowState::Ready { .. }) {
        return Vec::new();
    }
    let selected: Option<Verdict> = view
        .user_filter
        .terms
        .iter()
        .find(|t| t.field == Field::Verdict && t.is_eq())
        .and_then(|t| t.verdicts().and_then(|v| (v.len() == 1).then(|| v[0])));
    let any_verdict = view
        .user_filter
        .terms
        .iter()
        .any(|t| t.field == Field::Verdict);
    let netobserv = matches!(
        state,
        FlowState::Ready {
            kind: BackendKind::NetObserv,
            ..
        }
    );
    // Verdicts the server leaves out have no count here.
    let server = super::rows::SplitFilter::new(&view.pushed);
    let mut chips: Vec<(Option<Verdict>, SharedString, Option<usize>, bool)> =
        vec![(None, "All".into(), Some(view.rows.total()), !any_verdict)];
    for (verdict, count) in verdict_counts(view) {
        // "No reply" only where it happens (NetObserv) or was seen.
        if verdict == Verdict::NoReply && count == 0 && !netobserv {
            continue;
        }
        let label: SharedString = match verdict {
            Verdict::Forwarded => "Forwarded".into(),
            Verdict::Dropped => "Dropped".into(),
            _ => "No reply".into(),
        };
        let count = server.verdict_ok(verdict).then_some(count);
        chips.push((Some(verdict), label, count, selected == Some(verdict)));
    }
    chips
        .into_iter()
        .map(|(verdict, label, count, on)| {
            let weak = cx.entity().downgrade();
            h_flex()
                .id(SharedString::from(format!("flow-verdict-{label}")))
                .flex_none()
                .h(u(20.0))
                .px(u(7.0))
                .gap(u(5.0))
                .rounded(u(4.0))
                .cursor_pointer()
                .text_size(u(11.5))
                .map(|this| {
                    if on {
                        this.bg(colors.chip_selected_background)
                            .text_color(colors.chip_selected_text)
                            .border_1()
                            .border_color(colors.chip_selected_border)
                    } else {
                        this.bg(colors.chip_background)
                            .text_color(colors.text_muted)
                    }
                })
                .when_some(verdict, |this, v| {
                    this.child(
                        div()
                            .size(u(7.0))
                            .rounded_full()
                            .bg(widgets::verdict_color(v, colors)),
                    )
                })
                .child(label)
                .when_some(count, |this, count| {
                    this.child(
                        div()
                            .font_family(fonts::MONO)
                            .text_size(u(11.0))
                            .text_color(colors.text_dim)
                            .child(widgets::count(count)),
                    )
                })
                .on_click(move |_, _, cx| {
                    weak.update(cx, |this, cx| {
                        let mut filter = this.user_filter.clone();
                        filter.terms.retain(|t| t.field != Field::Verdict);
                        let mut text = filter.canonical();
                        if let Some(v) = verdict {
                            if !text.is_empty() {
                                text.push(' ');
                            }
                            text.push_str(&format!("verdict={}", v.key()));
                        }
                        this.set_query(text, cx);
                    })
                    .ok();
                })
                .into_any_element()
        })
        .collect()
}

fn suggestions(
    view: &NetworkFlowsView,
    colors: &Colors,
    cx: &mut Context<NetworkFlowsView>,
) -> AnyElement {
    let key = view
        .suggestions
        .first()
        .map(|s| {
            s.text
                .split(['=', '!', '<', '>'])
                .next()
                .unwrap_or_default()
                .to_string()
        })
        .unwrap_or_default();
    let values = view.suggestions.iter().any(|s| !s.text.ends_with('='));
    v_flex()
        .absolute()
        .top(u(42.0))
        .left(u(12.0))
        .w(u(320.0))
        .p(u(4.0))
        .rounded(u(7.0))
        .bg(colors.elevated)
        .border_1()
        .border_color(colors.border)
        .shadow_lg()
        .text_size(u(12.0))
        .child(
            div()
                .px(u(8.0))
                .py(u(4.0))
                .text_size(u(11.0))
                .text_color(colors.text_dim)
                .child(if values {
                    format!("{key} · seen in the buffer")
                } else {
                    "fields".to_string()
                }),
        )
        .children(view.suggestions.iter().enumerate().map(|(i, suggestion)| {
            let weak = cx.entity().downgrade();
            let highlighted = i == view.suggestion;
            h_flex()
                .id(("flow-suggestion", i))
                .px(u(8.0))
                .py(u(4.0))
                .gap(u(8.0))
                .rounded(u(4.0))
                .cursor_pointer()
                .when(highlighted, |this| this.bg(colors.chip_selected_background))
                .hover(|s| s.bg(colors.hover))
                .when_some(suggestion.verdict, |this, v| {
                    this.child(
                        div()
                            .size(u(7.0))
                            .rounded_full()
                            .bg(widgets::verdict_color(v, colors)),
                    )
                })
                .child(
                    div()
                        .flex_1()
                        .min_w_0()
                        .truncate()
                        .font_family(fonts::MONO)
                        .child(suggestion.label.clone()),
                )
                .when_some(suggestion.count, |this, n| {
                    this.child(
                        div()
                            .font_family(fonts::MONO)
                            .text_size(u(11.0))
                            .text_color(colors.text_dim)
                            .child(widgets::count(n as usize)),
                    )
                })
                .on_click(move |_, window, cx| {
                    weak.update(cx, |this, cx| {
                        this.suggestion = i;
                        this.accept_suggestion(window, cx);
                        this.input.read(cx).focus_handle(cx).focus(window, cx);
                    })
                    .ok();
                })
        }))
        .child(
            div()
                .px(u(8.0))
                .pt(u(5.0))
                .mt(u(3.0))
                .border_t_1()
                .border_color(colors.border_variant)
                .text_size(u(11.0))
                .text_color(colors.text_faint)
                .child("tab completes · src. and dst. pick a side"),
        )
        .into_any_element()
}

fn dropdown_label(label: String, active: bool, colors: &Colors) -> impl IntoElement {
    h_flex()
        .gap(u(4.0))
        .text_size(u(12.0))
        .text_color(if active {
            colors.chip_selected_text
        } else {
            colors.text_muted
        })
        .child(Icon::new(IconName::Folder).size(12.0))
        .child(label)
        .child(Icon::new(IconName::ChevronDown).size(11.0))
}

/// The namespace scope: the whole cluster, the active namespace, or one seen in the flows.
fn scope_menu(
    view: &NetworkFlowsView,
    colors: &Colors,
    cx: &mut Context<NetworkFlowsView>,
) -> AnyElement {
    let current = view.namespace.clone();
    let mut namespaces: Vec<String> = Vec::new();
    let active = kubyl_core::ActiveContext::global(cx);
    if active.cluster.as_ref().map(|c| &c.id) == Some(&view.cluster)
        && let Some(ns) = &active.namespace
    {
        namespaces.push(ns.to_string());
    }
    if let Some(service) = FlowService::global(cx)
        && let Some(stream) = service.read(cx).stream(&view.cluster, &view.pushed)
    {
        for (ns, _) in stream
            .buffer
            .seen()
            .values(Field::Namespace)
            .into_iter()
            .take(12)
        {
            if !namespaces.contains(&ns) {
                namespaces.push(ns);
            }
        }
    }
    let title = current.clone().unwrap_or_else(|| "All namespaces".into());
    let weak = cx.entity().downgrade();
    MenuButton::new("flows-scope")
        .ghost()
        .compact()
        .child(dropdown_label(title, current.is_some(), colors))
        .dropdown_menu(move |menu, _, _| {
            let pick = |value: Option<String>| {
                let weak = weak.clone();
                move |_: &gpui::ClickEvent, window: &mut Window, cx: &mut App| {
                    let value = value.clone();
                    weak.update(cx, |this, cx| {
                        this.namespace = value;
                        this.sync(cx);
                        let _ = window;
                    })
                    .ok();
                }
            };
            let mut menu = menu
                .max_h(px(360.0))
                .scrollable(true)
                .item(
                    PopupMenuItem::new("All namespaces")
                        .checked(current.is_none())
                        .on_click(pick(None)),
                )
                .separator();
            for ns in &namespaces {
                menu = menu.item(
                    PopupMenuItem::new(ns.clone())
                        .checked(current.as_deref() == Some(ns.as_str()))
                        .on_click(pick(Some(ns.clone()))),
                );
            }
            menu
        })
        .into_any_element()
}

/// `⌁ Hubble Relay kube-system/hubble-relay · v1.20.2 · 3/3 nodes`, with how it's reached in
/// the tooltip.
pub(super) fn backend_chip(
    view: &NetworkFlowsView,
    state: &FlowState,
    colors: &Colors,
    cx: &App,
) -> AnyElement {
    let _ = view;
    let _ = cx;
    let (color, text, tooltip): (gpui::Hsla, String, String) = match state {
        FlowState::Ready {
            kind,
            status,
            capabilities,
            ..
        } => {
            let mut text = format!("{} {}", kind.label(), status.endpoint);
            if let Some(version) = &status.version {
                text.push_str(&format!(" · v{version}"));
            }
            if let Some((connected, total)) = status.nodes {
                text.push_str(&format!(" · {connected}/{total} nodes"));
            }
            if !capabilities.single_flows {
                text.push_str(" · metrics only");
            }
            let mut tooltip = format!("{} · {}", status.endpoint, status.via);
            if let Some((held, max)) = status.buffered {
                tooltip.push_str(&format!(
                    "\n{} of {} flows buffered in Relay",
                    widgets::count(held as usize),
                    widgets::count(max as usize)
                ));
            }
            tooltip.push_str(&format!("\nHistory: {}", capabilities.history));
            for note in &status.notes {
                tooltip.push_str(&format!("\n{note}"));
            }
            let color = if status.notes.is_empty() && capabilities.single_flows {
                colors.green
            } else {
                colors.yellow
            };
            (color, text, tooltip)
        }
        FlowState::Connecting { kind, .. } => (
            colors.text_dim,
            format!("{} · connecting…", kind.label()),
            "Connecting to the flow source".into(),
        ),
        FlowState::Failed { kind, error, .. } => {
            let text = match error {
                crate::provider::ProviderError::Forbidden { .. } => {
                    format!("{} · forbidden", kind.label())
                }
                _ => format!("{} · unavailable", kind.label()),
            };
            (colors.red, text, error.to_string())
        }
        FlowState::Detecting => (
            colors.text_dim,
            "Looking for a flow source…".into(),
            "Kubyl looks for Hubble Relay, Calico Whisker and NetObserv".into(),
        ),
        FlowState::NoBackend(_) => (
            colors.text_dim,
            "no flow source".into(),
            "Nothing Kubyl can read flows from".into(),
        ),
        FlowState::Off => (
            colors.text_dim,
            "turned off in settings".into(),
            "netflow.clusters.<cluster>.backend is off".into(),
        ),
        FlowState::NotConnected => (
            colors.text_dim,
            "not connected".into(),
            "The cluster isn't connected".into(),
        ),
    };
    let tooltip: SharedString = tooltip.into();
    h_flex()
        .id("flows-backend")
        .flex_shrink(1.0)
        .min_w(u(120.0))
        .h(u(22.0))
        .px(u(7.0))
        .gap(u(5.0))
        .rounded(u(4.0))
        .bg(colors.chip_background)
        .text_size(u(11.5))
        .text_color(if color == colors.red {
            colors.red
        } else {
            colors.text_muted
        })
        .child(Icon::new(IconName::Waypoints).size(11.0).color(color))
        .child(div().min_w_0().truncate().child(text))
        .tooltip(move |window, cx| {
            gpui_component::tooltip::Tooltip::new(tooltip.clone()).build(window, cx)
        })
        .into_any_element()
}

/// "also: NetObserv", a menu that picks another backend (writes settings).
pub(super) fn other_backends(
    view: &NetworkFlowsView,
    state: &FlowState,
    colors: &Colors,
    _cx: &mut Context<NetworkFlowsView>,
) -> Option<AnyElement> {
    let detection = state.detection()?;
    let current = match state {
        FlowState::Ready { kind, .. }
        | FlowState::Connecting { kind, .. }
        | FlowState::Failed { kind, .. } => Some(*kind),
        _ => None,
    };
    let others: Vec<BackendKind> = detection
        .candidates
        .iter()
        .map(|c| c.kind())
        .filter(|k| Some(*k) != current)
        .collect();
    if others.is_empty() {
        return None;
    }
    let label = format!(
        "also: {}",
        others
            .iter()
            .map(|k| k.label())
            .collect::<Vec<_>>()
            .join(", ")
    );
    let cluster = view.cluster.clone();
    Some(
        MenuButton::new("flows-other-backends")
            .ghost()
            .compact()
            .child(
                h_flex()
                    .gap(u(4.0))
                    .text_size(u(12.0))
                    .text_color(colors.text_dim)
                    .child(label)
                    .child(Icon::new(IconName::ChevronDown).size(11.0)),
            )
            .dropdown_menu(move |mut menu, _, _| {
                for kind in &others {
                    let cluster = cluster.clone();
                    let kind = *kind;
                    menu = menu.item(
                        PopupMenuItem::new(format!("Use {}", kind.label())).on_click(
                            move |_, _, cx| {
                                FlowService::use_backend(&cluster, kind, cx);
                            },
                        ),
                    );
                }
                menu
            })
            .into_any_element(),
    )
}

/// `● live · 184 flows/s`, `paused · 1,204 new`, `reconnecting…`.
pub(super) fn live_chip(
    view: &NetworkFlowsView,
    state: &FlowState,
    colors: &Colors,
    cx: &mut Context<NetworkFlowsView>,
) -> Option<AnyElement> {
    if !matches!(state, FlowState::Ready { .. }) {
        return None;
    }
    let (status, rate, caught_up) = super::stream_status(view, cx)?;
    let waiting = view.rows.pending.len();
    let (color, text, tooltip): (gpui::Hsla, String, Option<String>) = if view.paused {
        (
            colors.yellow,
            format!("paused · {} new", widgets::count(waiting)),
            None,
        )
    } else if waiting > 0 {
        (
            colors.yellow,
            format!("{} new above", widgets::count(waiting)),
            None,
        )
    } else {
        match status {
            StreamStatus::Starting => (colors.text_dim, "starting…".into(), None),
            StreamStatus::Retrying(err) => {
                (colors.orange, "reconnecting…".into(), Some(err.to_string()))
            }
            StreamStatus::Live if !caught_up => (colors.accent, "loading history…".into(), None),
            StreamStatus::Live => {
                let polled =
                    matches!(state, FlowState::Ready { capabilities, .. } if !capabilities.live);
                let rate = if rate >= 10.0 {
                    format!("{rate:.0}")
                } else {
                    format!("{rate:.1}")
                };
                (
                    colors.green,
                    if polled {
                        format!("polling · {rate} flows/s")
                    } else {
                        format!("live · {rate} flows/s")
                    },
                    None,
                )
            }
        }
    };
    let weak = cx.entity().downgrade();
    let clickable = view.paused || waiting > 0;
    Some(
        h_flex()
            .id("flows-live")
            .flex_none()
            .h(u(22.0))
            .px(u(7.0))
            .gap(u(5.0))
            .rounded(u(4.0))
            .bg(colors.chip_background)
            .text_size(u(11.5))
            .text_color(color)
            .child(div().size(u(7.0)).rounded_full().bg(color))
            .child(text)
            .when(clickable, |this| {
                this.cursor_pointer().on_click(move |_, _, cx| {
                    weak.update(cx, |this, cx| this.show_newest(cx)).ok();
                })
            })
            .when_some(tooltip, |this, tooltip| {
                let tooltip: SharedString = tooltip.into();
                this.tooltip(move |window, cx| {
                    gpui_component::tooltip::Tooltip::new(tooltip.clone()).build(window, cx)
                })
            })
            .into_any_element(),
    )
}

/// The "…" menu: look again, connect again, copy the filter.
pub(super) fn more_menu(
    view: &NetworkFlowsView,
    colors: &Colors,
    cx: &mut Context<NetworkFlowsView>,
) -> AnyElement {
    let _ = (colors, cx);
    let cluster = view.cluster.clone();
    MenuButton::new("flows-more")
        .ghost()
        .compact()
        .child(Icon::new(IconName::Ellipsis).size(14.0))
        .dropdown_menu(move |menu, _, _| {
            let look = cluster.clone();
            let again = cluster.clone();
            menu.item(PopupMenuItem::new("Look for a flow source again").on_click(
                move |_, _, cx| {
                    if let Some(service) = FlowService::global(cx) {
                        service.update(cx, |s, cx| s.redetect(&look, cx));
                    }
                },
            ))
            .item(
                PopupMenuItem::new("Connect again").on_click(move |_, _, cx| {
                    if let Some(service) = FlowService::global(cx) {
                        service.update(cx, |s, cx| s.reconnect(&again, cx));
                    }
                }),
            )
        })
        .into_any_element()
}
