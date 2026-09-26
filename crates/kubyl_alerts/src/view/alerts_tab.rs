//! The Alerts tab: summary line, filters, the grouped table and the details pane.

use std::ops::Range;

use gpui::{
    AnyElement, App, Context, Focusable as _, FontWeight, IntoElement, SharedString, Window, div,
    prelude::*, px, uniform_list,
};
use gpui_component::button::{Button as MenuButton, ButtonVariants as _};
use gpui_component::input::Input;
use gpui_component::menu::{DropdownMenu as _, PopupMenuItem};
use jiff::Timestamp;
use kubyl_core::{ClusterId, ColumnDef, ColumnWidth};
use kubyl_ui::{ActiveColors, Chip, Colors, Icon, IconName, fonts, h_flex, sizes, u, v_flex};

use super::rows::{self, Row, SEVERITY_CHIPS, STATE_CHIPS};
use super::widgets::{self, severity_color};
use super::{AlertsView, states};
use crate::merge::Heartbeat;
use crate::model::{Alert, AlertState, TargetKind};
use crate::service::Counts;
use crate::settings::GroupBy;

const ROW_HEIGHT: f32 = 31.0;

pub(crate) fn target_icon(kind: TargetKind) -> IconName {
    match kind {
        TargetKind::Pod => IconName::Box,
        TargetKind::Deployment | TargetKind::ReplicaSet => IconName::Layers,
        TargetKind::StatefulSet | TargetKind::DaemonSet => IconName::Database,
        TargetKind::Job | TargetKind::CronJob => IconName::Play,
        TargetKind::HorizontalPodAutoscaler => IconName::Activity,
        TargetKind::PersistentVolumeClaim => IconName::HardDrive,
        TargetKind::Node => IconName::Server,
        TargetKind::ClusterOperator => IconName::Blocks,
        TargetKind::Service => IconName::Network,
        TargetKind::Namespace => IconName::Folder,
    }
}

impl AlertsView {
    fn columns(&self) -> Vec<ColumnDef> {
        let flex = |weight: f32, min: f32| ColumnWidth::Flex { weight, min };
        let details = self.details_open && self.selected.is_some();
        let mut columns = vec![ColumnDef::new(
            "severity",
            "Severity",
            ColumnWidth::Fixed(88.0),
        )];
        if self.cluster.is_none() {
            columns.push(ColumnDef::new(
                "cluster",
                "Cluster",
                ColumnWidth::Fixed(130.0),
            ));
        }
        columns.extend([
            ColumnDef::new("alert", "Alert", flex(1.3, 150.0)),
            ColumnDef::new("state", "State", ColumnWidth::Fixed(86.0)),
            ColumnDef::new("since", "Since", ColumnWidth::Fixed(62.0)),
            ColumnDef::new("summary", "Summary", flex(1.6, 120.0)),
            ColumnDef::new("target", "Target", flex(1.2, 110.0)),
        ]);
        if !details && self.sources > 1 {
            columns.push(ColumnDef::new(
                "source",
                "Source",
                ColumnWidth::Fixed(150.0),
            ));
        }
        if !details {
            columns.extend([
                ColumnDef::new("namespace", "Namespace", ColumnWidth::Fixed(110.0)),
                ColumnDef::new("receivers", "Receivers", ColumnWidth::Fixed(130.0)),
                ColumnDef::new("labels", "Labels", flex(1.2, 120.0)),
            ]);
        }
        columns
    }

    pub(crate) fn render_alerts_tab(
        &mut self,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        if let Some(blocking) = self.blocking_state(cx) {
            return self.render_blocking(blocking, cx);
        }
        let colors = cx.colors().clone();
        let summary = self.render_summary(cx);
        let filters = self.render_filters(window, cx);
        let all_clear = self.alerts.iter().all(|a| a.state == AlertState::Resolved)
            && self.filters.is_empty()
            && self.cluster.is_some();
        let body = if all_clear {
            self.render_all_clear(cx)
        } else if self.rows.is_empty() {
            widgets::empty(
                if self.alerts.is_empty() {
                    "No alerts."
                } else {
                    "No alerts match the filters."
                },
                &colors,
            )
        } else {
            let columns = self.columns();
            v_flex()
                .flex_1()
                .min_h_0()
                .child(widgets::header(&columns, &colors))
                .child(
                    uniform_list(
                        "alerts-rows",
                        self.rows.len(),
                        cx.processor(|this, range: Range<usize>, _, cx| {
                            this.render_rows(range, cx)
                        }),
                    )
                    .flex_1()
                    .track_scroll(&self.scroll),
                )
                .into_any_element()
        };
        let problems = self.render_cluster_problems(cx);
        let details = (self.details_open && !all_clear)
            .then(|| self.selected_entry())
            .flatten()
            .map(|entry| self.render_details(entry, window, cx));
        v_flex()
            .size_full()
            .when(!all_clear, |this| this.child(summary).child(filters))
            .child(
                self.focus_area()
                    .items_start()
                    .child(
                        v_flex()
                            .flex_1()
                            .min_w_0()
                            .h_full()
                            .child(body)
                            .children(problems),
                    )
                    .children(details),
            )
            .into_any_element()
    }

    fn render_summary(&self, cx: &mut Context<Self>) -> AnyElement {
        let colors = cx.colors().clone();
        let counts = Counts::of(&self.alerts);
        let mut parts: Vec<AnyElement> = Vec::new();
        let bold = |text: String, color| {
            div()
                .font_weight(FontWeight::SEMIBOLD)
                .text_color(color)
                .child(text)
                .into_any_element()
        };
        let sep = || {
            div()
                .text_color(colors.text_faint)
                .child("·")
                .into_any_element()
        };
        let mut firing_parts = Vec::new();
        if counts.critical > 0 {
            firing_parts.push(bold(format!("{} critical", counts.critical), colors.red));
        }
        if counts.warning > 0 {
            firing_parts.push(bold(format!("{} warning", counts.warning), colors.yellow));
        }
        if counts.info > 0 {
            firing_parts.push(bold(format!("{} info", counts.info), colors.accent));
        }
        if counts.other > 0 {
            firing_parts.push(bold(format!("{} other", counts.other), colors.text_muted));
        }
        if firing_parts.is_empty() {
            parts.push(
                div()
                    .text_color(colors.green)
                    .child("Nothing firing")
                    .into_any_element(),
            );
        } else {
            for (i, part) in firing_parts.into_iter().enumerate() {
                if i > 0 {
                    parts.push(sep());
                }
                parts.push(part);
            }
            parts.push(
                div()
                    .text_color(colors.text_muted)
                    .child("firing")
                    .into_any_element(),
            );
        }
        if counts.pending > 0 {
            parts.push(sep());
            parts.push(bold(format!("{} pending", counts.pending), colors.yellow));
        }
        let mut quiet = Vec::new();
        if counts.silenced > 0 {
            quiet.push(format!("{} silenced", counts.silenced));
        }
        if counts.inhibited > 0 {
            quiet.push(format!("{} inhibited", counts.inhibited));
        }
        if !quiet.is_empty() {
            // Listed after the active alerts; clicking hides or shows them.
            let shown = self.filters.show_suppressed;
            let tooltip: SharedString = if shown {
                "Firing, but nobody is notified. Click to hide them.".into()
            } else {
                "Hidden. Click to list them after the active alerts.".into()
            };
            parts.push(sep());
            parts.push(
                h_flex()
                    .id("summary-suppressed")
                    .gap(u(4.0))
                    .cursor_pointer()
                    .text_color(colors.text_muted)
                    .hover(|s| s.text_color(colors.text))
                    .child(Icon::new(IconName::BellOff).size(12.0))
                    .child(quiet.join(" · "))
                    .when(!shown, |this| {
                        this.child(div().text_color(colors.text_dim).child("(hidden)"))
                    })
                    .tooltip(move |window, cx| {
                        gpui_component::tooltip::Tooltip::new(tooltip.clone()).build(window, cx)
                    })
                    .on_click(cx.listener(|this, _, _, cx| {
                        this.filters.show_suppressed = !this.filters.show_suppressed;
                        this.save_options(cx);
                        this.rebuild(cx);
                    }))
                    .into_any_element(),
            );
        }
        let now = Timestamp::now();
        let mut right: Vec<AnyElement> = Vec::new();
        if let Some(cluster) = &self.cluster
            && let Some(state) = self.state(cluster, cx)
        {
            if let Some(heartbeat) = &state.heartbeat {
                right.push(heartbeat_line(heartbeat, now, &colors, false));
            }
            let failing = state.failing_rules();
            if failing > 0 {
                right.push(
                    h_flex()
                        .gap(u(5.0))
                        .text_color(colors.red)
                        .child(Icon::new(IconName::CircleX).size(12.0).color(colors.red))
                        .child(format!(
                            "{failing} rule{} fail{} to evaluate",
                            if failing == 1 { "" } else { "s" },
                            if failing == 1 { "s" } else { "" }
                        ))
                        .into_any_element(),
                );
            }
            let stale = state.error.clone();
            let checked = state
                .checked_at
                .map(|t| format!("checked {} ago", widgets::ago(Some(t), now)));
            match stale {
                Some(error) => {
                    let tooltip = error.clone();
                    right.push(
                        h_flex()
                            .id("alerts-stale")
                            .gap(u(5.0))
                            .text_color(colors.yellow)
                            .child(
                                Icon::new(IconName::TriangleAlert)
                                    .size(12.0)
                                    .color(colors.yellow),
                            )
                            .child(format!(
                                "stale{}",
                                checked
                                    .as_ref()
                                    .map(|c| format!(" · {c}"))
                                    .unwrap_or_default()
                            ))
                            .tooltip(move |window, cx| {
                                gpui_component::tooltip::Tooltip::new(tooltip.clone())
                                    .build(window, cx)
                            })
                            .into_any_element(),
                    );
                }
                None => {
                    if let Some(checked) = checked {
                        right.push(
                            div()
                                .text_color(colors.text_dim)
                                .child(checked)
                                .into_any_element(),
                        );
                    }
                }
            }
        }
        h_flex()
            .flex_none()
            .h(u(34.0))
            .px(u(14.0))
            .gap(u(6.0))
            .border_b_1()
            .border_color(colors.border_variant)
            .text_size(u(12.5))
            .whitespace_nowrap()
            .overflow_hidden()
            .children(parts)
            .child(div().flex_1())
            .child(h_flex().gap(u(16.0)).children(right))
            .into_any_element()
    }

    fn render_filters(&self, window: &mut Window, cx: &mut Context<Self>) -> AnyElement {
        let colors = cx.colors().clone();
        let counts = rows::chip_counts(&self.alerts, &self.filters);
        let focused = self
            .filter_input
            .read(cx)
            .focus_handle(cx)
            .is_focused(window);
        let error = self.filters.query.error().map(str::to_string);
        let input = div()
            .id("alerts-filter")
            .flex_none()
            .w(u(210.0))
            .h(u(sizes::CONTROL))
            .px(u(8.0))
            .flex()
            .items_center()
            .gap(u(7.0))
            .rounded(u(5.0))
            .bg(colors.input_background)
            .border_1()
            .border_color(if error.is_some() {
                colors.red
            } else if focused {
                colors.accent
            } else {
                colors.border
            })
            .font_family(fonts::MONO)
            .text_size(u(11.5))
            .child(Icon::new(IconName::Funnel).size(12.0))
            .child(
                div()
                    .flex_1()
                    .min_w_0()
                    .child(Input::new(&self.filter_input).appearance(false)),
            )
            .when_some(error, |this, error| {
                let error: SharedString = error.into();
                this.tooltip(move |window, cx| {
                    gpui_component::tooltip::Tooltip::new(error.clone()).build(window, cx)
                })
            });
        let chip = |id: SharedString,
                    label: String,
                    count: usize,
                    dot: Option<gpui::Hsla>,
                    selected: bool| {
            let mut chip = Chip::new(format!("{label} {count}")).selected(selected);
            if let Some(dot) = dot {
                chip = chip.dot(dot);
            }
            div().id(id).cursor_pointer().child(chip)
        };
        let mut row = h_flex()
            .flex_none()
            .h(u(38.0))
            .px(u(12.0))
            .gap(u(6.0))
            .border_b_1()
            .border_color(colors.border_variant)
            .overflow_hidden()
            .child(input);
        if let Some(object) = &self.filters.object {
            row = row.child(
                div()
                    .id("alerts-object-filter")
                    .cursor_pointer()
                    .child(Chip::new(object.label()).mono().selected(true).removable())
                    .on_click(cx.listener(|this, _, _, cx| {
                        this.filters.object = None;
                        this.rebuild(cx);
                    })),
            );
        }
        for key in SEVERITY_CHIPS {
            let count = counts.severities.get(key).copied().unwrap_or(0);
            let selected = self.filters.severities.contains(key);
            if count == 0 && !selected {
                continue;
            }
            let severity = crate::model::Severity::from_value(key, &Default::default());
            let label = capitalize(key);
            row = row.child(
                chip(
                    format!("sev-chip-{key}").into(),
                    label,
                    count,
                    Some(severity_color(&severity, &colors)),
                    selected,
                )
                .on_click(cx.listener(move |this, _, _, cx| {
                    if !this.filters.severities.remove(key) {
                        this.filters.severities.insert(key.to_string());
                    }
                    this.save_options(cx);
                    this.rebuild(cx);
                })),
            );
        }
        row = row.child(div().w(u(1.0)).h(u(16.0)).mx(u(4.0)).bg(colors.border));
        for key in STATE_CHIPS {
            let count = counts.states.get(key).copied().unwrap_or(0);
            let selected = self.filters.states.contains(key);
            if count == 0 && !selected {
                continue;
            }
            row = row.child(
                chip(
                    format!("state-chip-{key}").into(),
                    capitalize(key),
                    count,
                    None,
                    selected,
                )
                .on_click(cx.listener(move |this, _, _, cx| {
                    if !this.filters.states.remove(key) {
                        this.filters.states.insert(key.to_string());
                    }
                    this.save_options(cx);
                    this.rebuild(cx);
                })),
            );
        }
        row.child(div().flex_1())
            .child(self.namespace_menu(cx))
            .child(self.receiver_menu(cx))
            .child(self.group_menu(cx))
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
            .child(label)
            .child(Icon::new(IconName::ChevronDown).size(11.0))
    }

    fn namespace_menu(&self, cx: &mut Context<Self>) -> AnyElement {
        let colors = cx.colors().clone();
        let mut namespaces: Vec<String> = self
            .alerts
            .iter()
            .filter_map(|a| a.namespace().map(str::to_string))
            .collect();
        namespaces.sort();
        namespaces.dedup();
        let current = self.filters.namespace.clone();
        let title = match &current {
            None => "All namespaces".to_string(),
            Some(ns) if ns.is_empty() => "Cluster".to_string(),
            Some(ns) => ns.clone(),
        };
        let active_ns = kubyl_core::ActiveContext::global(cx)
            .namespace
            .as_ref()
            .map(|n| n.to_string());
        let weak = cx.entity().downgrade();
        MenuButton::new("alerts-namespace")
            .ghost()
            .compact()
            .child(Self::dropdown_label(title, current.is_some(), &colors))
            .dropdown_menu(move |menu, _, _| {
                let mut menu = menu.max_h(px(420.0)).scrollable(true);
                let pick = |value: Option<String>, follow: bool| {
                    let weak = weak.clone();
                    move |_: &gpui::ClickEvent, _: &mut Window, cx: &mut App| {
                        let value = value.clone();
                        weak.update(cx, |this, cx| {
                            this.filters.namespace = value;
                            this.options.active_namespace_only = follow;
                            this.save_options(cx);
                            this.rebuild(cx);
                        })
                        .ok();
                    }
                };
                menu = menu.item(
                    PopupMenuItem::new("All namespaces")
                        .checked(current.is_none())
                        .on_click(pick(None, false)),
                );
                if let Some(active) = &active_ns {
                    menu = menu.item(
                        PopupMenuItem::new(format!("The active namespace ({active})"))
                            .on_click(pick(Some(active.clone()), true)),
                    );
                }
                menu = menu.item(
                    PopupMenuItem::new("Cluster (no namespace)")
                        .checked(current.as_deref() == Some(""))
                        .on_click(pick(Some(String::new()), false)),
                );
                menu = menu.separator();
                for ns in &namespaces {
                    menu = menu.item(
                        PopupMenuItem::new(ns.clone())
                            .checked(current.as_deref() == Some(ns.as_str()))
                            .on_click(pick(Some(ns.clone()), false)),
                    );
                }
                menu
            })
            .into_any_element()
    }

    fn receiver_menu(&self, cx: &mut Context<Self>) -> AnyElement {
        let colors = cx.colors().clone();
        let mut receivers: Vec<String> = self
            .alerts
            .iter()
            .flat_map(|a| a.receivers.iter().cloned())
            .collect();
        receivers.sort();
        receivers.dedup();
        let current = self.filters.receiver.clone();
        if receivers.is_empty() && current.is_none() {
            return div().into_any_element();
        }
        let title = current
            .clone()
            .map(|r| format!("Receiver: {r}"))
            .unwrap_or_else(|| "All receivers".into());
        let weak = cx.entity().downgrade();
        MenuButton::new("alerts-receiver")
            .ghost()
            .compact()
            .child(Self::dropdown_label(title, current.is_some(), &colors))
            .dropdown_menu(move |menu, _, _| {
                let pick = |value: Option<String>| {
                    let weak = weak.clone();
                    move |_: &gpui::ClickEvent, _: &mut Window, cx: &mut App| {
                        let value = value.clone();
                        weak.update(cx, |this, cx| {
                            this.filters.receiver = value;
                            this.rebuild(cx);
                        })
                        .ok();
                    }
                };
                let mut menu = menu
                    .item(
                        PopupMenuItem::new("All receivers")
                            .checked(current.is_none())
                            .on_click(pick(None)),
                    )
                    .separator();
                for receiver in &receivers {
                    menu = menu.item(
                        PopupMenuItem::new(receiver.clone())
                            .checked(current.as_deref() == Some(receiver.as_str()))
                            .on_click(pick(Some(receiver.clone()))),
                    );
                }
                menu
            })
            .into_any_element()
    }

    fn group_menu(&self, cx: &mut Context<Self>) -> AnyElement {
        let colors = cx.colors().clone();
        let current = self.options.group_by;
        let weak = cx.entity().downgrade();
        MenuButton::new("alerts-group")
            .ghost()
            .compact()
            .child(Self::dropdown_label(
                format!("Group: {}", current.label()),
                false,
                &colors,
            ))
            .dropdown_menu(move |mut menu, _, _| {
                for by in GroupBy::ALL {
                    let weak = weak.clone();
                    menu = menu.item(
                        PopupMenuItem::new(capitalize(by.label()))
                            .checked(by == current)
                            .on_click(move |_, _, cx| {
                                weak.update(cx, |this, cx| {
                                    this.options.group_by = by;
                                    this.save_options(cx);
                                    this.rebuild(cx);
                                })
                                .ok();
                            }),
                    );
                }
                menu
            })
            .into_any_element()
    }

    fn render_rows(&mut self, range: Range<usize>, cx: &mut Context<Self>) -> Vec<AnyElement> {
        let colors = cx.colors().clone();
        let columns = self.columns();
        let selected = self.selected_index();
        let now = Timestamp::now();
        range
            .filter_map(|index| {
                let row = self.rows.get(index)?.clone();
                Some(match row {
                    Row::Group {
                        key,
                        label,
                        count,
                        severity,
                        oldest,
                        collapsed,
                    } => widgets::row(("alert-group", index), false, ROW_HEIGHT, &colors)
                        .gap(u(8.0))
                        .cursor_pointer()
                        .child(
                            Icon::new(if collapsed {
                                IconName::ChevronRight
                            } else {
                                IconName::ChevronDown
                            })
                            .size(12.0)
                            .color(colors.text_dim),
                        )
                        .child(
                            div()
                                .font_family(fonts::MONO)
                                .font_weight(FontWeight::SEMIBOLD)
                                .text_size(u(12.5))
                                .child(label),
                        )
                        .child(
                            div()
                                .px(u(5.0))
                                .rounded(u(4.0))
                                .bg(colors.chip_background)
                                .font_family(fonts::MONO)
                                .text_size(u(11.0))
                                .text_color(colors.text_muted)
                                .child(format!("×{count}")),
                        )
                        .child(widgets::severity_pill(&severity, &colors))
                        .when_some(oldest, |this, oldest| {
                            this.child(
                                div()
                                    .text_color(colors.text_dim)
                                    .child(format!("· oldest {}", widgets::ago(Some(oldest), now))),
                            )
                        })
                        .on_click(cx.listener(move |this, _, _, cx| this.toggle_group(&key, cx)))
                        .into_any_element(),
                    Row::Suppressed {
                        silenced,
                        inhibited,
                    } => {
                        let mut text = format!("{silenced} silenced");
                        if inhibited > 0 {
                            text.push_str(&format!(" · {inhibited} inhibited"));
                        }
                        widgets::row(("alert-suppressed", index), false, ROW_HEIGHT, &colors)
                            .gap(u(8.0))
                            .text_color(colors.text_dim)
                            .child(Icon::new(IconName::BellOff).size(12.0))
                            .child(text)
                            .child(widgets::text_button("show-suppressed", "Show", &colors, {
                                let weak = cx.entity().downgrade();
                                move |_, _, cx| {
                                    weak.update(cx, |this, cx| {
                                        this.filters.show_suppressed = true;
                                        this.save_options(cx);
                                        this.rebuild(cx);
                                    })
                                    .ok();
                                }
                            }))
                            .into_any_element()
                    }
                    Row::ResolvedHeader { count, collapsed } => {
                        widgets::row(("alert-resolved", index), false, ROW_HEIGHT, &colors)
                            .gap(u(8.0))
                            .cursor_pointer()
                            .text_color(colors.text_dim)
                            .child(
                                Icon::new(if collapsed {
                                    IconName::ChevronRight
                                } else {
                                    IconName::ChevronDown
                                })
                                .size(12.0),
                            )
                            .child(
                                Icon::new(IconName::CircleCheck)
                                    .size(12.0)
                                    .color(colors.green),
                            )
                            .child(format!("{count} resolved in the last 15 min"))
                            .on_click(cx.listener(|this, _, _, cx| {
                                this.resolved_open = !this.resolved_open;
                                this.rebuild(cx);
                            }))
                            .into_any_element()
                    }
                    Row::Alert {
                        index: alert_index,
                        resolved,
                        nested,
                    } => {
                        let (alert, cluster) = if resolved {
                            (
                                self.resolved.get(alert_index)?.clone(),
                                self.resolved_clusters.get(alert_index)?.clone(),
                            )
                        } else {
                            (
                                self.alerts.get(alert_index)?.clone(),
                                self.alert_clusters.get(alert_index)?.clone(),
                            )
                        };
                        let is_selected = selected == Some(index);
                        widgets::row(("alert-row", index), is_selected, ROW_HEIGHT, &colors)
                            .when(resolved, |this| this.opacity(0.65))
                            .when(alert.state.suppressed(), |this| this.opacity(0.8))
                            .children(columns.iter().map(|def| {
                                widgets::column_cell(def).child(self.cell(
                                    &alert,
                                    &cluster,
                                    def.id.as_ref(),
                                    nested,
                                    now,
                                    &colors,
                                    cx,
                                ))
                            }))
                            .on_click(cx.listener(
                                move |this, event: &gpui::ClickEvent, window, cx| {
                                    this.focus.focus(window, cx);
                                    this.select_row(index, cx);
                                    if event.click_count() == 2 {
                                        this.details_open = true;
                                    }
                                },
                            ))
                            .into_any_element()
                    }
                })
            })
            .collect()
    }

    /// "Silenced by alice@example.com: “…” · ends in 2h 51m", or who inhibits it.
    fn suppressed_by(
        &self,
        alert: &Alert,
        cluster: &ClusterId,
        now: Timestamp,
        cx: &App,
    ) -> Option<SharedString> {
        match alert.state {
            AlertState::Silenced => {
                let silences = self
                    .state(cluster, cx)
                    .map(|s| s.silences.clone())
                    .unwrap_or_default();
                let lines: Vec<String> = alert
                    .silenced_by
                    .iter()
                    .map(|id| match silences.iter().find(|s| &s.id == id) {
                        Some(silence) => {
                            let ends = silence
                                .ends_at
                                .map(|t| {
                                    format!(
                                        " · ends in {} ({})",
                                        widgets::short_duration(t.duration_since(now).as_secs()),
                                        widgets::local_and_utc(t)
                                    )
                                })
                                .unwrap_or_default();
                            format!(
                                "Silenced by {}: “{}”{ends}",
                                silence.created_by, silence.comment
                            )
                        }
                        None => format!(
                            "Silenced by silence {}",
                            id.chars().take(8).collect::<String>()
                        ),
                    })
                    .collect();
                Some(if lines.is_empty() {
                    "Firing, but silenced: nobody is notified.".into()
                } else {
                    format!("Firing, but nobody is notified.\n{}", lines.join("\n")).into()
                })
            }
            AlertState::Inhibited => {
                let by: Vec<String> = alert
                    .inhibited_by
                    .iter()
                    .map(|fp| {
                        self.alerts
                            .iter()
                            .find(|a| &a.fingerprint == fp)
                            .map(|a| a.name.clone())
                            .unwrap_or_else(|| fp.clone())
                    })
                    .collect();
                Some(
                    format!(
                        "Firing, but inhibited by {}: nobody is notified.",
                        by.join(", ")
                    )
                    .into(),
                )
            }
            _ => None,
        }
    }

    #[allow(clippy::too_many_arguments)]
    fn cell(
        &self,
        alert: &Alert,
        cluster: &ClusterId,
        column: &str,
        nested: bool,
        now: Timestamp,
        colors: &Colors,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        match column {
            "severity" => widgets::severity_pill(&alert.severity, colors),
            "cluster" => div()
                .truncate()
                .text_color(colors.text_muted)
                .child(Self::cluster_name(cluster, cx))
                .into_any_element(),
            "alert" => div()
                .truncate()
                .when(nested, |this| this.pl(u(14.0)))
                .font_family(fonts::MONO)
                .text_size(u(12.0))
                .child(alert.name.clone())
                .into_any_element(),
            "state" => {
                let label = widgets::state_label(alert.state, false, colors);
                match self.suppressed_by(alert, cluster, now, cx) {
                    Some(tooltip) => div()
                        .id(SharedString::from(format!("state-{}", alert.fingerprint)))
                        .child(label)
                        .tooltip(move |window, cx| {
                            gpui_component::tooltip::Tooltip::new(tooltip.clone()).build(window, cx)
                        })
                        .into_any_element(),
                    None => label,
                }
            }
            "since" => {
                let (text, at) = if alert.state == AlertState::Resolved {
                    (widgets::ago(alert.ends_at, now), alert.ends_at)
                } else {
                    (widgets::ago(alert.since(), now), alert.since())
                };
                let approx = alert.starts_approx && alert.state != AlertState::Pending;
                let tooltip: Option<SharedString> = at.map(|t| {
                    let mut tip = widgets::local_and_utc(t);
                    if alert.state == AlertState::Resolved {
                        tip = format!("Resolved {tip}");
                    } else if approx {
                        tip.push_str(" · approximate (Prometheus' activeAt plus the rule's for)");
                    }
                    tip.into()
                });
                div()
                    .id(SharedString::from(format!("since-{}", alert.fingerprint)))
                    .font_family(fonts::MONO)
                    .text_size(u(11.5))
                    .text_color(colors.text_muted)
                    .child(if approx { format!("~{text}") } else { text })
                    .when_some(tooltip, |this, tooltip| {
                        this.tooltip(move |window, cx| {
                            gpui_component::tooltip::Tooltip::new(tooltip.clone()).build(window, cx)
                        })
                    })
                    .into_any_element()
            }
            "summary" => div()
                .truncate()
                .text_color(colors.text_muted)
                .child(alert.summary().to_string())
                .into_any_element(),
            "target" => match &alert.target {
                Some(target) => {
                    let open_target = target.clone();
                    let cluster = cluster.clone();
                    h_flex()
                        .min_w_0()
                        .gap(u(5.0))
                        .child(
                            Icon::new(target_icon(target.kind))
                                .size(12.0)
                                .color(colors.accent),
                        )
                        .child(
                            widgets::link(
                                SharedString::from(format!("target-{}", alert.fingerprint)),
                                target.name.clone(),
                                colors,
                                move |_, window, cx| {
                                    crate::actions::open_target(
                                        &cluster,
                                        &open_target,
                                        false,
                                        window,
                                        cx,
                                    )
                                },
                            )
                            .font_family(fonts::MONO)
                            .text_size(u(11.5)),
                        )
                        .into_any_element()
                }
                None => div().into_any_element(),
            },
            "namespace" => div()
                .truncate()
                .text_color(colors.text_muted)
                .child(alert.namespace().unwrap_or("—").to_string())
                .into_any_element(),
            "source" => div()
                .truncate()
                .text_color(colors.text_muted)
                .child(alert.source.clone())
                .into_any_element(),
            "receivers" => div()
                .truncate()
                .text_color(colors.text_muted)
                .child(alert.receivers.join(", "))
                .into_any_element(),
            "labels" => {
                let shown = ["alertname", "severity", "namespace"];
                h_flex()
                    .gap(u(4.0))
                    .overflow_hidden()
                    .children(
                        alert
                            .labels
                            .iter()
                            .filter(|(k, _)| !shown.contains(&k.as_str()))
                            .take(4)
                            .map(|(k, v)| widgets::label_chip(k, v, colors)),
                    )
                    .into_any_element()
            }
            _ => div().into_any_element(),
        }
    }

    fn render_all_clear(&self, cx: &mut Context<Self>) -> AnyElement {
        let colors = cx.colors().clone();
        let now = Timestamp::now();
        let Some(cluster) = &self.cluster else {
            return widgets::empty("No alerts firing.", &colors);
        };
        let Some(state) = self.state(cluster, cx) else {
            return widgets::empty("No alerts firing.", &colors);
        };
        let counts = state.counts();
        let mut lines: Vec<AnyElement> = Vec::new();
        if let Some(heartbeat) = &state.heartbeat {
            lines.push(heartbeat_line(heartbeat, now, &colors, true));
        }
        let rules = state.rule_count();
        if rules > 0 {
            let failing = state.failing_rules();
            lines.push(
                div()
                    .text_color(if failing > 0 {
                        colors.red
                    } else {
                        colors.text_muted
                    })
                    .child(if failing > 0 {
                        format!("{rules} alerting rules · {failing} fail to evaluate")
                    } else {
                        format!("{rules} alerting rules evaluate · all healthy")
                    })
                    .into_any_element(),
            );
        }
        let mut quiet = Vec::new();
        if counts.pending > 0 {
            quiet.push(format!("{} pending", counts.pending));
        }
        if counts.silenced > 0 {
            quiet.push(format!("{} silenced", counts.silenced));
        }
        if !quiet.is_empty() {
            lines.push(
                widgets::text_button("all-clear-show", quiet.join(" · "), &colors, {
                    let weak = cx.entity().downgrade();
                    move |_, _, cx| {
                        weak.update(cx, |this, cx| {
                            this.filters.show_suppressed = true;
                            this.filters.states = ["pending", "silenced", "inhibited"]
                                .iter()
                                .map(|s| s.to_string())
                                .collect();
                            this.rebuild(cx);
                        })
                        .ok();
                    }
                })
                .into_any_element(),
            );
        }
        let source = state
            .sources
            .first()
            .map(|s| format!(" · Alertmanager {}", s.label()))
            .or_else(|| state.rules_source.as_ref().map(|r| format!(" · {r}")))
            .unwrap_or_default();
        if let Some(checked) = state.checked_at {
            lines.push(
                div()
                    .text_color(colors.text_dim)
                    .child(format!(
                        "Last check {} ago{source}",
                        widgets::ago(Some(checked), now)
                    ))
                    .into_any_element(),
            );
        }
        v_flex()
            .flex_1()
            .items_center()
            .justify_center()
            .gap(u(8.0))
            .text_size(u(12.5))
            .child(
                div()
                    .size(u(56.0))
                    .rounded_full()
                    .border_1()
                    .border_color(colors.green)
                    .bg(colors.green.opacity(0.12))
                    .flex()
                    .items_center()
                    .justify_center()
                    .child(Icon::new(IconName::Check).size(26.0).color(colors.green)),
            )
            .child(
                div()
                    .pt(u(6.0))
                    .text_size(u(16.0))
                    .font_weight(FontWeight::SEMIBOLD)
                    .child("No alerts firing"),
            )
            .children(lines)
            .into_any_element()
    }

    /// All clusters: clusters without a source, or with errors, at the bottom.
    fn render_cluster_problems(&self, cx: &mut Context<Self>) -> Option<AnyElement> {
        if self.cluster.is_some() {
            return None;
        }
        let colors = cx.colors().clone();
        let mut lines = Vec::new();
        for cluster in self.clusters(cx) {
            let name = Self::cluster_name(&cluster, cx);
            let Some(state) = self.state(&cluster, cx) else {
                lines.push((
                    name,
                    "looking for alert sources…".to_string(),
                    colors.text_dim,
                ));
                continue;
            };
            let (text, color) = match (&state.phase, &state.error) {
                (crate::service::Phase::NoSource, _) => (
                    "no Alertmanager or Prometheus found".to_string(),
                    colors.text_dim,
                ),
                (crate::service::Phase::Disabled, _) => {
                    ("alerts are off".to_string(), colors.text_dim)
                }
                (crate::service::Phase::Unknown | crate::service::Phase::Discovering, _) => {
                    ("looking for alert sources…".to_string(), colors.text_dim)
                }
                (_, Some(error)) => (error.to_string(), colors.red),
                _ => continue,
            };
            lines.push((name, text, color));
        }
        if lines.is_empty() {
            return None;
        }
        Some(
            v_flex()
                .flex_none()
                .px(u(14.0))
                .py(u(10.0))
                .gap(u(4.0))
                .border_t_1()
                .border_color(colors.border_variant)
                .text_size(u(12.0))
                .child(
                    div()
                        .text_size(u(11.0))
                        .font_weight(FontWeight::SEMIBOLD)
                        .text_color(colors.text_dim)
                        .child("CLUSTERS WITHOUT ALERTS"),
                )
                .children(lines.into_iter().map(|(name, text, color)| {
                    h_flex()
                        .gap(u(8.0))
                        .child(div().w(u(160.0)).truncate().child(name))
                        .child(div().truncate().text_color(color).child(text))
                }))
                .into_any_element(),
        )
    }

    pub(crate) fn render_blocking(
        &self,
        blocking: states::Blocking,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let Some(cluster) = self.cluster.clone() else {
            return self.focus_area().into_any_element();
        };
        let state = states::render(blocking, &cluster, self.state(&cluster, cx), cx);
        self.focus_area().child(state).into_any_element()
    }
}

/// `Heartbeat OK · Watchdog received 14s ago`, or what's wrong.
pub(crate) fn heartbeat_line(
    heartbeat: &Heartbeat,
    now: Timestamp,
    colors: &Colors,
    long: bool,
) -> AnyElement {
    let (icon, color, text) = match heartbeat {
        Heartbeat::Unknown => return div().into_any_element(),
        Heartbeat::Ok(at) => (
            IconName::Activity,
            colors.text_muted,
            if long {
                format!(
                    "Heartbeat OK · Watchdog received {} ago",
                    widgets::ago(Some(*at), now)
                )
            } else {
                format!("Heartbeat OK · {} ago", widgets::ago(Some(*at), now))
            },
        ),
        Heartbeat::Stale(at) => (
            IconName::TriangleAlert,
            colors.yellow,
            format!(
                "Heartbeat stale · last received {} ago",
                widgets::ago(Some(*at), now)
            ),
        ),
        Heartbeat::NotDelivered => (
            IconName::TriangleAlert,
            colors.red,
            "Prometheus isn't delivering alerts to this Alertmanager".to_string(),
        ),
        Heartbeat::NoAlertmanagerConfigured => (
            IconName::TriangleAlert,
            colors.red,
            "Prometheus has no Alertmanager configured".to_string(),
        ),
        Heartbeat::FiringInPrometheus => (
            IconName::Activity,
            colors.text_muted,
            "Heartbeat fires in Prometheus (no Alertmanager to check delivery)".to_string(),
        ),
    };
    h_flex()
        .gap(u(5.0))
        .text_color(color)
        .child(Icon::new(icon).size(12.0).color(color))
        .child(text)
        .into_any_element()
}

pub(crate) fn capitalize(text: &str) -> String {
    let mut chars = text.chars();
    match chars.next() {
        Some(first) => first.to_uppercase().collect::<String>() + chars.as_str(),
        None => String::new(),
    }
}
