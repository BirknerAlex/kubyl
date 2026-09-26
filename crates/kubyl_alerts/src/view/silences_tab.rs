//! The Silences tab: active and pending silences, expired ones (last 24 h) collapsed; edit,
//! extend, expire, recreate and copy as `amtool`.

use std::ops::Range;

use gpui::{
    AnyElement, Context, Focusable as _, IntoElement, SharedString, Window, div, prelude::*,
    uniform_list,
};
use gpui_component::input::Input;
use jiff::Timestamp;
use kubyl_core::{ColumnDef, ColumnWidth};
use kubyl_ui::{ActiveColors, Button, Colors, Icon, IconName, fonts, h_flex, sizes, u, v_flex};

use super::{AlertsView, SilenceEntry, widgets};
use crate::matchers;
use crate::model::Silence;

const ROW_HEIGHT: f32 = 34.0;
/// Expired silences older than this aren't listed.
const EXPIRED_WINDOW: i64 = 86_400;

/// A row of the Silences table.
#[derive(Clone)]
pub(crate) enum SilenceRow {
    Silence(SilenceEntry),
    ExpiredHeader(usize),
}

pub(crate) fn is_expired(silence: &Silence) -> bool {
    silence.state == "expired"
}

/// `alertname="X" namespace="y"` against a free-text filter.
fn matches_filter(silence: &Silence, filter: &str) -> bool {
    if filter.is_empty() {
        return true;
    }
    let filter = filter.to_lowercase();
    matchers::format(&silence.matchers)
        .to_lowercase()
        .contains(&filter)
        || silence.comment.to_lowercase().contains(&filter)
        || silence.created_by.to_lowercase().contains(&filter)
}

impl AlertsView {
    pub(crate) fn silence_rows(&self, cx: &gpui::App) -> Vec<SilenceRow> {
        let filter = self.silence_filter.read(cx).value().trim().to_string();
        let now = Timestamp::now();
        let mut live: Vec<SilenceEntry> = Vec::new();
        let mut expired: Vec<SilenceEntry> = Vec::new();
        for entry in self.silences(cx) {
            if !matches_filter(&entry.silence, &filter) {
                continue;
            }
            if is_expired(&entry.silence) {
                let recent = entry
                    .silence
                    .ends_at
                    .is_none_or(|t| now.duration_since(t).as_secs() < EXPIRED_WINDOW);
                if recent {
                    expired.push(entry);
                }
            } else {
                live.push(entry);
            }
        }
        // Active first, then pending; soonest end first.
        live.sort_by(|a, b| {
            (a.silence.state != "active")
                .cmp(&(b.silence.state != "active"))
                .then_with(|| a.silence.ends_at.cmp(&b.silence.ends_at))
        });
        expired.sort_by_key(|e| std::cmp::Reverse(e.silence.ends_at));
        let mut rows: Vec<SilenceRow> = live.into_iter().map(SilenceRow::Silence).collect();
        if !expired.is_empty() {
            rows.push(SilenceRow::ExpiredHeader(expired.len()));
            if self.expired_open {
                rows.extend(expired.into_iter().map(SilenceRow::Silence));
            }
        }
        rows
    }

    /// Alerts a silence matches now (in its cluster).
    pub(crate) fn silence_matches(&self, entry: &SilenceEntry) -> usize {
        self.alerts
            .iter()
            .zip(&self.alert_clusters)
            .filter(|(a, c)| *c == &entry.cluster && entry.silence.matches(&a.labels))
            .count()
    }

    pub(crate) fn selected_silence_entry(&self, cx: &gpui::App) -> Option<SilenceEntry> {
        let id = self.selected_silence.as_ref()?;
        self.silences(cx).into_iter().find(|e| &e.silence.id == id)
    }

    pub(crate) fn move_silence(&mut self, delta: isize, cx: &mut Context<Self>) {
        let rows = self.silence_rows(cx);
        let ids: Vec<String> = rows
            .iter()
            .filter_map(|r| match r {
                SilenceRow::Silence(e) => Some(e.silence.id.clone()),
                _ => None,
            })
            .collect();
        if ids.is_empty() {
            return;
        }
        let current = self
            .selected_silence
            .as_ref()
            .and_then(|id| ids.iter().position(|i| i == id));
        let next = match current {
            None => 0,
            Some(pos) => (pos as isize + delta).clamp(0, ids.len() as isize - 1) as usize,
        };
        self.selected_silence = Some(ids[next].clone());
        if let Some(index) = rows
            .iter()
            .position(|r| matches!(r, SilenceRow::Silence(e) if e.silence.id == ids[next]))
        {
            self.silences_scroll
                .scroll_to_item(index, gpui::ScrollStrategy::Nearest);
        }
        cx.notify();
    }

    fn silence_columns(&self) -> Vec<ColumnDef> {
        let flex = |weight: f32, min: f32| ColumnWidth::Flex { weight, min };
        let mut columns = vec![ColumnDef::new("state", "State", ColumnWidth::Fixed(76.0))];
        if self.cluster.is_none() {
            columns.push(ColumnDef::new(
                "cluster",
                "Cluster",
                ColumnWidth::Fixed(120.0),
            ));
        }
        columns.extend([
            ColumnDef::new("matchers", "Matchers", flex(2.0, 180.0)),
            ColumnDef::new("comment", "Comment", flex(1.4, 120.0)),
            ColumnDef::new("created", "Created by", ColumnWidth::Fixed(150.0)),
            ColumnDef::new("starts", "Starts", ColumnWidth::Fixed(92.0)),
            ColumnDef::new("ends", "Ends", ColumnWidth::Fixed(118.0)),
            ColumnDef::new("matches", "Matches", ColumnWidth::Fixed(64.0)),
            ColumnDef::new("actions", "", ColumnWidth::Fixed(110.0)),
        ]);
        columns
    }

    pub(crate) fn render_silences_tab(
        &mut self,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let colors = cx.colors().clone();
        if let Some(blocking) = self.blocking_state(cx) {
            return self.render_blocking(blocking, cx);
        }
        let has_am = self
            .clusters(cx)
            .iter()
            .any(|c| self.state(c, cx).is_some_and(|s| s.has_alertmanager()));
        if !has_am {
            return self.focus_area().child(widgets::empty(
                "Silences need an Alertmanager. This cluster's alerts come from Prometheus only.",
                &colors,
            )).into_any_element();
        }
        let focused = self
            .silence_filter
            .read(cx)
            .focus_handle(cx)
            .is_focused(window);
        let read_only = self.cluster.as_ref().is_some_and(|c| self.read_only(c, cx));
        let new_cluster = self.cluster.clone();
        let toolbar = h_flex()
            .flex_none()
            .h(u(40.0))
            .px(u(12.0))
            .gap(u(8.0))
            .border_b_1()
            .border_color(colors.border_variant)
            .child(
                div()
                    .flex_none()
                    .w(u(240.0))
                    .h(u(sizes::CONTROL))
                    .px(u(8.0))
                    .flex()
                    .items_center()
                    .gap(u(7.0))
                    .rounded(u(5.0))
                    .bg(colors.input_background)
                    .border_1()
                    .border_color(if focused {
                        colors.accent
                    } else {
                        colors.border
                    })
                    .text_size(u(12.0))
                    .child(Icon::new(IconName::Funnel).size(12.0))
                    .child(
                        div()
                            .flex_1()
                            .min_w_0()
                            .child(Input::new(&self.silence_filter).appearance(false)),
                    ),
            )
            .child(div().flex_1())
            .when(!read_only, |this| {
                this.when_some(new_cluster, |this, cluster| {
                    this.child(
                        Button::new("new-silence")
                            .primary()
                            .icon(IconName::Plus)
                            .label("New silence…")
                            .on_click(move |_, window, cx| {
                                crate::silence::open_new(&cluster, window, cx)
                            }),
                    )
                })
            });
        let rows = self.silence_rows(cx);
        let columns = self.silence_columns();
        let body = if rows.is_empty() {
            widgets::empty("No silences.", &colors)
        } else {
            let count = rows.len();
            v_flex()
                .flex_1()
                .min_h_0()
                .child(widgets::header(&columns, &colors))
                .child(
                    uniform_list(
                        "silence-rows",
                        count,
                        cx.processor(move |this, range: Range<usize>, _, cx| {
                            let rows = this.silence_rows(cx);
                            this.render_silence_rows(&rows, range, cx)
                        }),
                    )
                    .flex_1()
                    .track_scroll(&self.silences_scroll),
                )
                .into_any_element()
        };
        v_flex()
            .size_full()
            .child(toolbar)
            .child(self.focus_area().flex_col().child(body))
            .into_any_element()
    }

    fn render_silence_rows(
        &mut self,
        rows: &[SilenceRow],
        range: Range<usize>,
        cx: &mut Context<Self>,
    ) -> Vec<AnyElement> {
        let colors = cx.colors().clone();
        let columns = self.silence_columns();
        let now = Timestamp::now();
        range
            .filter_map(|index| {
                Some(match rows.get(index)?.clone() {
                    SilenceRow::ExpiredHeader(count) => {
                        let open = self.expired_open;
                        widgets::row(("silence-expired", index), false, ROW_HEIGHT, &colors)
                            .gap(u(8.0))
                            .cursor_pointer()
                            .text_color(colors.text_dim)
                            .child(
                                Icon::new(if open {
                                    IconName::ChevronDown
                                } else {
                                    IconName::ChevronRight
                                })
                                .size(12.0),
                            )
                            .child(format!("Expired in the last 24 h · {count}"))
                            .on_click(cx.listener(|this, _, _, cx| {
                                this.expired_open = !this.expired_open;
                                cx.notify();
                            }))
                            .into_any_element()
                    }
                    SilenceRow::Silence(entry) => {
                        let selected = self.selected_silence.as_ref() == Some(&entry.silence.id);
                        let id = entry.silence.id.clone();
                        widgets::row(("silence-row", index), selected, ROW_HEIGHT, &colors)
                            .when(is_expired(&entry.silence), |this| this.opacity(0.6))
                            .children(columns.iter().map(|def| {
                                widgets::column_cell(def).child(self.silence_cell(
                                    &entry,
                                    def.id.as_ref(),
                                    now,
                                    &colors,
                                    cx,
                                ))
                            }))
                            .on_click(cx.listener(
                                move |this, event: &gpui::ClickEvent, window, cx| {
                                    this.focus.focus(window, cx);
                                    this.selected_silence = Some(id.clone());
                                    if event.click_count() == 2
                                        && let Some(entry) = this.selected_silence_entry(cx)
                                    {
                                        crate::silence::open_edit(
                                            &entry.cluster,
                                            &entry.silence,
                                            window,
                                            cx,
                                        );
                                    }
                                    cx.notify();
                                },
                            ))
                            .into_any_element()
                    }
                })
            })
            .collect()
    }

    fn silence_cell(
        &self,
        entry: &SilenceEntry,
        column: &str,
        now: Timestamp,
        colors: &Colors,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let silence = &entry.silence;
        match column {
            "state" => {
                let color = match silence.state.as_str() {
                    "active" => colors.green,
                    "pending" => colors.yellow,
                    _ => colors.text_dim,
                };
                h_flex()
                    .gap(u(6.0))
                    .text_color(color)
                    .child(kubyl_ui::StatusDot::new(color))
                    .child(silence.state.clone())
                    .into_any_element()
            }
            "cluster" => div()
                .truncate()
                .text_color(colors.text_muted)
                .child(Self::cluster_name(&entry.cluster, cx))
                .into_any_element(),
            "matchers" => h_flex()
                .gap(u(4.0))
                .overflow_hidden()
                .children(silence.matchers.iter().map(|m| {
                    div()
                        .flex_none()
                        .px(u(6.0))
                        .py(u(1.0))
                        .rounded(u(4.0))
                        .bg(colors.chip_background)
                        .font_family(fonts::MONO)
                        .text_size(u(11.0))
                        .text_color(colors.text_muted)
                        .child(format!("{} {} {}", m.name, m.op.symbol(), m.value))
                }))
                .into_any_element(),
            "comment" => div()
                .truncate()
                .child(silence.comment.clone())
                .into_any_element(),
            "created" => div()
                .truncate()
                .text_color(colors.text_muted)
                .child(silence.created_by.clone())
                .into_any_element(),
            "starts" => {
                let text = match silence.starts_at {
                    Some(t) if t > now => format!(
                        "in {}",
                        widgets::short_duration(t.duration_since(now).as_secs())
                    ),
                    Some(t) => format!("{} ago", widgets::ago(Some(t), now)),
                    None => String::new(),
                };
                div()
                    .text_color(colors.text_muted)
                    .text_size(u(12.0))
                    .child(text)
                    .into_any_element()
            }
            "ends" => {
                let text = match silence.ends_at {
                    Some(t) if t > now => format!(
                        "ends in {}",
                        widgets::short_duration(t.duration_since(now).as_secs())
                    ),
                    Some(t) => format!("ended {} ago", widgets::ago(Some(t), now)),
                    None => String::new(),
                };
                let tooltip: Option<SharedString> =
                    silence.ends_at.map(|t| widgets::local_and_utc(t).into());
                div()
                    .id(SharedString::from(format!("silence-ends-{}", silence.id)))
                    .font_family(fonts::MONO)
                    .text_size(u(11.5))
                    .child(text)
                    .when_some(tooltip, |this, tooltip| {
                        this.tooltip(move |window, cx| {
                            gpui_component::tooltip::Tooltip::new(tooltip.clone()).build(window, cx)
                        })
                    })
                    .into_any_element()
            }
            "matches" => div()
                .font_family(fonts::MONO)
                .text_size(u(11.5))
                .child(if is_expired(silence) {
                    "—".to_string()
                } else {
                    self.silence_matches(entry).to_string()
                })
                .into_any_element(),
            "actions" => {
                if self.read_only(&entry.cluster, cx) {
                    return div().into_any_element();
                }
                if is_expired(silence) {
                    let (cluster, silence) = (entry.cluster.clone(), silence.clone());
                    return widgets::text_button(
                        SharedString::from(format!("recreate-{}", entry.silence.id)),
                        "Recreate…",
                        colors,
                        move |_, window, cx| {
                            crate::silence::open_recreate(&cluster, &silence, window, cx)
                        },
                    )
                    .text_size(u(12.0))
                    .into_any_element();
                }
                let extend = |hours: i64| {
                    let (cluster, silence) = (entry.cluster.clone(), entry.silence.clone());
                    widgets::text_button(
                        SharedString::from(format!("extend-{hours}-{}", entry.silence.id)),
                        format!("+{hours}h"),
                        colors,
                        move |_, window, cx| {
                            crate::silence::extend(&cluster, &silence, hours, window, cx)
                        },
                    )
                    .text_size(u(12.0))
                };
                let one = extend(1);
                let four = extend(4);
                let (cluster, silence) = (entry.cluster.clone(), entry.silence.clone());
                h_flex()
                    .gap(u(10.0))
                    .child(one)
                    .child(four)
                    .child(
                        div()
                            .id(SharedString::from(format!("expire-{}", entry.silence.id)))
                            .cursor_pointer()
                            .text_size(u(12.0))
                            .text_color(colors.text_muted)
                            .hover(|s| s.text_color(colors.red))
                            .child("Expire…")
                            .on_click(move |_, window, cx| {
                                crate::silence::confirm_expire(&cluster, &silence, window, cx)
                            }),
                    )
                    .into_any_element()
            }
            _ => div().into_any_element(),
        }
    }
}
