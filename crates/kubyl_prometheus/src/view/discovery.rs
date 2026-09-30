//! Service Discovery: what discovery found per scrape pool, which targets relabeling kept and
//! which it dropped, with the labels before and after.

use std::ops::Range;

use gpui::{
    AnyElement, Context, Entity, Focusable as _, FontWeight, IntoElement, Subscription,
    UniformListScrollHandle, Window, div, prelude::*, uniform_list,
};
use gpui_component::input::{InputEvent, InputState};
use kubyl_core::{ColumnDef, ColumnWidth};
use kubyl_ui::{ActiveColors, fonts, h_flex, u, v_flex};

use super::{PrometheusView, widgets};
use crate::model::{Pool, Targets};

const ROW_HEIGHT: f32 = 31.0;
/// Targets drawn per list until "Show all".
const CAP: usize = 50;

pub(crate) struct State {
    filter: Entity<InputState>,
    pub(super) selected: Option<String>,
    show_all: bool,
    scroll: UniformListScrollHandle,
}

impl State {
    pub(crate) fn new(window: &mut Window, cx: &mut Context<PrometheusView>) -> Self {
        Self {
            filter: cx.new(|cx| InputState::new(window, cx).placeholder("Filter pools")),
            selected: None,
            show_all: false,
            scroll: UniformListScrollHandle::new(),
        }
    }

    pub(crate) fn subscriptions(
        &self,
        window: &mut Window,
        cx: &mut Context<PrometheusView>,
    ) -> Vec<Subscription> {
        vec![
            cx.subscribe_in(&self.filter, window, |_, _, event: &InputEvent, _, cx| {
                if matches!(event, InputEvent::Change) {
                    cx.notify();
                }
            }),
        ]
    }
}

/// The pools whose name contains every word of `filter`.
pub(crate) fn filter_pools(targets: &Targets, filter: &str) -> Vec<Pool> {
    let words: Vec<String> = filter.split_whitespace().map(str::to_lowercase).collect();
    targets
        .pools()
        .into_iter()
        .filter(|p| {
            let name = p.name.to_lowercase();
            words.iter().all(|w| name.contains(w))
        })
        .collect()
}

fn columns() -> Vec<ColumnDef> {
    vec![
        ColumnDef::new(
            "pool",
            "Pool",
            ColumnWidth::Flex {
                weight: 1.0,
                min: 160.0,
            },
        )
        .mono(),
        ColumnDef::new("active", "Active", ColumnWidth::Fixed(84.0)),
        ColumnDef::new("dropped", "Dropped", ColumnWidth::Fixed(84.0)),
    ]
}

impl PrometheusView {
    pub(super) fn render_discovery(
        &mut self,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let colors = cx.colors().clone();
        let Some(targets) = self.targets.data.clone() else {
            return match &self.targets.error {
                Some(error) => {
                    widgets::empty(format!("Couldn't read the targets: {error}"), &colors)
                }
                None => widgets::empty("Loading service discovery…", &colors),
            };
        };
        let filter = self.discovery_tab.filter.read(cx).value().to_string();
        let pools = filter_pools(&targets, &filter);
        let focused = self
            .discovery_tab
            .filter
            .read(cx)
            .focus_handle(cx)
            .is_focused(window);
        let toolbar = widgets::toolbar(&colors)
            .child(div().child(format!(
                "{} pools · {} active · {} dropped targets",
                targets.pools().len(),
                targets.active.len(),
                targets.dropped_counts.values().sum::<usize>()
            )))
            .child(div().flex_1())
            .child(widgets::filter_box(
                &self.discovery_tab.filter,
                240.0,
                focused,
                &colors,
            ));
        let cols = columns();
        let list = if pools.is_empty() {
            widgets::empty("No pools match.", &colors)
        } else {
            let count = pools.len();
            let pools = std::sync::Arc::new(pools);
            let for_rows = pools.clone();
            v_flex()
                .flex_1()
                .min_h_0()
                .child(widgets::header(&cols, &colors))
                .child(
                    uniform_list(
                        "pool-rows",
                        count,
                        cx.processor(move |this, range: Range<usize>, _, cx| {
                            this.render_pool_rows(&for_rows, range, cx)
                        }),
                    )
                    .flex_1()
                    .track_scroll(&self.discovery_tab.scroll),
                )
                .into_any_element()
        };
        let details = self
            .discovery_tab
            .selected
            .clone()
            .and_then(|name| self.render_pool_details(&targets, &name, cx));
        v_flex()
            .size_full()
            .children(
                self.targets
                    .error
                    .as_ref()
                    .map(|e| Self::error_banner(e, &colors)),
            )
            .child(toolbar)
            .child(
                div()
                    .flex()
                    .flex_1()
                    .min_h_0()
                    .min_w_0()
                    .items_start()
                    .child(v_flex().flex_1().min_w_0().h_full().child(list))
                    .children(details),
            )
            .into_any_element()
    }

    fn render_pool_rows(
        &mut self,
        pools: &[Pool],
        range: Range<usize>,
        cx: &mut Context<Self>,
    ) -> Vec<AnyElement> {
        let colors = cx.colors().clone();
        let cols = columns();
        range
            .filter_map(|index| {
                let pool = pools.get(index)?;
                let selected = self.discovery_tab.selected.as_deref() == Some(pool.name.as_str());
                let name = pool.name.clone();
                let active_color = if pool.down > 0 {
                    colors.red
                } else {
                    colors.text_muted
                };
                let cells: Vec<AnyElement> = vec![
                    div().truncate().child(pool.name.clone()).into_any_element(),
                    div()
                        .text_color(active_color)
                        .child(format!("{}/{}", pool.up, pool.total()))
                        .into_any_element(),
                    div()
                        .text_color(colors.text_muted)
                        .child(pool.dropped.to_string())
                        .into_any_element(),
                ];
                Some(
                    widgets::row(("pool", index), selected, ROW_HEIGHT, &colors)
                        .children(cols.iter().zip(cells).map(|(def, cell)| {
                            widgets::column_cell(def)
                                .when(def.mono, |d| d.font_family(fonts::MONO).text_size(u(11.5)))
                                .child(cell)
                        }))
                        .on_click(cx.listener(move |this, _, _, cx| {
                            this.discovery_tab.selected = Some(name.clone());
                            this.discovery_tab.show_all = false;
                            cx.notify();
                        }))
                        .into_any_element(),
                )
            })
            .collect()
    }

    fn render_pool_details(
        &self,
        targets: &Targets,
        name: &str,
        cx: &mut Context<Self>,
    ) -> Option<AnyElement> {
        let colors = cx.colors().clone();
        let active: Vec<_> = targets.active.iter().filter(|t| t.pool == name).collect();
        let dropped: Vec<_> = targets.dropped.iter().filter(|d| d.pool == name).collect();
        let dropped_total = targets
            .dropped_counts
            .get(name)
            .copied()
            .unwrap_or(dropped.len());
        if active.is_empty() && dropped_total == 0 {
            return None;
        }
        let show_all = self.discovery_tab.show_all;
        let cap = if show_all { usize::MAX } else { CAP };
        let more = active.len().saturating_sub(CAP) + dropped.len().saturating_sub(CAP);
        Some(
            v_flex()
                .id("pool-details")
                .flex_none()
                .w(u(560.0))
                .h_full()
                .overflow_y_scroll()
                .border_l_1()
                .border_color(colors.border_variant)
                .child(
                    v_flex()
                        .px(u(14.0))
                        .py(u(12.0))
                        .gap(u(4.0))
                        .border_b_1()
                        .border_color(colors.border_variant)
                        .child(div().font_weight(FontWeight::SEMIBOLD).child(name.to_string()))
                        .child(
                            div()
                                .text_size(u(12.0))
                                .text_color(colors.text_dim)
                                .child(format!(
                                    "{} active · {} dropped by relabeling",
                                    active.len(),
                                    dropped_total
                                )),
                        ),
                )
                .child(
                    widgets::section(format!("Active targets ({})", active.len()), &colors).children(
                        active.iter().take(cap).map(|target| {
                            v_flex()
                                .gap(u(4.0))
                                .pb(u(8.0))
                                .child(
                                    h_flex()
                                        .gap(u(8.0))
                                        .child(widgets::health_label(target.health, &colors))
                                        .child(
                                            div()
                                                .min_w_0()
                                                .truncate()
                                                .font_family(fonts::MONO)
                                                .text_size(u(11.5))
                                                .child(target.url.clone()),
                                        ),
                                )
                                .child(label_block("Discovered", &target.discovered, &colors))
                                .child(label_block("Target", &target.labels, &colors))
                        }),
                    ),
                )
                .when(dropped_total > 0, |this| {
                    this.child(
                        widgets::section(format!("Dropped targets ({dropped_total})"), &colors)
                            .children(dropped.iter().take(cap).map(|d| {
                                label_block("Discovered", &d.discovered, &colors).pb(u(8.0))
                            }))
                            .when(dropped.len() < dropped_total, |this| {
                                this.child(div().text_size(u(11.5)).text_color(colors.text_dim).child(
                                    format!(
                                        "The server lists {} of {dropped_total} dropped targets.",
                                        dropped.len()
                                    ),
                                ))
                            }),
                    )
                })
                .when(more > 0 && !show_all, |this| {
                    this.child(
                        div().p(u(14.0)).child(widgets::button(
                            "pool-show-all",
                            None,
                            format!("Show {more} more"),
                            false,
                            &colors,
                            cx.listener(|this, _, _, cx| {
                                this.discovery_tab.show_all = true;
                                cx.notify();
                            }),
                        )),
                    )
                })
                .into_any_element(),
        )
    }
}

/// `Discovered` and its label chips.
fn label_block(
    title: &'static str,
    labels: &crate::model::Labels,
    colors: &kubyl_ui::Colors,
) -> gpui::Div {
    v_flex()
        .gap(u(3.0))
        .child(
            div()
                .text_size(u(10.5))
                .text_color(colors.text_dim)
                .child(title),
        )
        .child(if labels.is_empty() {
            div()
                .text_size(u(11.5))
                .text_color(colors.text_dim)
                .child("none")
        } else {
            widgets::chips(labels, colors)
        })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::{Dropped, Labels};

    #[test]
    fn filters_pools_by_name() {
        let targets = Targets {
            dropped: vec![
                Dropped {
                    pool: "serviceMonitor/kube-system/coredns/0".into(),
                    discovered: Labels::new(),
                },
                Dropped {
                    pool: "serviceMonitor/monitoring/node-exporter/0".into(),
                    discovered: Labels::new(),
                },
            ],
            dropped_counts: [
                ("serviceMonitor/kube-system/coredns/0".to_string(), 1),
                ("serviceMonitor/monitoring/node-exporter/0".to_string(), 1),
            ]
            .into(),
            ..Default::default()
        };
        assert_eq!(filter_pools(&targets, "").len(), 2);
        let found = filter_pools(&targets, "CORE dns");
        assert_eq!(found.len(), 1);
        assert_eq!(found[0].dropped, 1);
        assert!(filter_pools(&targets, "nothing").is_empty());
    }
}
