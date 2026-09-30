//! Target Health: scrape targets by pool with state, labels, last scrape and error, and a
//! details panel with the target's labels and the labels discovery found.

use std::collections::HashSet;
use std::ops::Range;
use std::sync::Arc;

use gpui::{
    AnyElement, Context, Entity, Focusable as _, FontWeight, IntoElement, Subscription,
    UniformListScrollHandle, Window, div, prelude::*, uniform_list,
};
use gpui_component::input::{InputEvent, InputState};
use jiff::Timestamp;
use kubyl_core::{ColumnDef, ColumnWidth};
use kubyl_ui::{ActiveColors, Chip, Icon, IconName, fonts, u, v_flex};

use super::{PrometheusView, widgets};
use crate::model::{Health, Pool, Target, Targets};

const ROW_HEIGHT: f32 = 31.0;

/// A row of the list.
#[derive(Clone, Debug, PartialEq)]
pub(crate) enum Row {
    Pool(Pool),
    /// An index into `Targets::active`.
    Target(usize),
}

pub(crate) struct State {
    filter: Entity<InputState>,
    health: Option<Health>,
    collapsed: HashSet<String>,
    /// Pool and scrape URL of the selected target.
    pub(super) selected: Option<(String, String)>,
    scroll: UniformListScrollHandle,
    rows: Arc<Vec<Row>>,
}

impl State {
    pub(crate) fn new(window: &mut Window, cx: &mut Context<PrometheusView>) -> Self {
        Self {
            filter: cx.new(|cx| {
                InputState::new(window, cx).placeholder("Filter by pool, endpoint, label or error")
            }),
            health: None,
            collapsed: HashSet::new(),
            selected: None,
            scroll: UniformListScrollHandle::new(),
            rows: Arc::default(),
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

/// Whether `target` contains every word of `filter` (case-insensitive) somewhere.
pub(crate) fn matches(target: &Target, filter: &str) -> bool {
    let words: Vec<String> = filter.split_whitespace().map(str::to_lowercase).collect();
    if words.is_empty() {
        return true;
    }
    let mut haystack = format!("{} {}", target.pool, target.url);
    for (k, v) in &target.labels {
        haystack.push_str(&format!(" {k}={v}"));
    }
    if let Some(error) = &target.last_error {
        haystack.push(' ');
        haystack.push_str(error);
    }
    let haystack = haystack.to_lowercase();
    words.iter().all(|w| haystack.contains(w))
}

/// The list: a header per pool with its targets below it unless the pool is collapsed. Only
/// pools with a matching target show.
pub(crate) fn build_rows(
    targets: &Targets,
    filter: &str,
    health: Option<Health>,
    collapsed: &HashSet<String>,
) -> Vec<Row> {
    let mut rows = Vec::new();
    let mut start = 0;
    while start < targets.active.len() {
        let pool = &targets.active[start].pool;
        let end = targets.active[start..]
            .iter()
            .position(|t| &t.pool != pool)
            .map_or(targets.active.len(), |n| start + n);
        let shown: Vec<usize> = (start..end)
            .filter(|&i| {
                let t = &targets.active[i];
                health.is_none_or(|h| t.health == h) && matches(t, filter)
            })
            .collect();
        if !shown.is_empty() {
            let mut header = Pool {
                name: pool.clone(),
                up: 0,
                down: 0,
                unknown: 0,
                dropped: 0,
            };
            for &i in &shown {
                match targets.active[i].health {
                    Health::Up => header.up += 1,
                    Health::Down => header.down += 1,
                    Health::Unknown => header.unknown += 1,
                }
            }
            rows.push(Row::Pool(header));
            if !collapsed.contains(pool) {
                rows.extend(shown.into_iter().map(Row::Target));
            }
        }
        start = end;
    }
    rows
}

fn columns() -> Vec<ColumnDef> {
    vec![
        ColumnDef::new("state", "State", ColumnWidth::Fixed(84.0)),
        ColumnDef::new(
            "endpoint",
            "Endpoint",
            ColumnWidth::Flex {
                weight: 2.0,
                min: 160.0,
            },
        )
        .mono(),
        ColumnDef::new(
            "labels",
            "Labels",
            ColumnWidth::Flex {
                weight: 3.0,
                min: 160.0,
            },
        )
        .mono(),
        ColumnDef::new("scrape", "Last scrape", ColumnWidth::Fixed(110.0)),
        ColumnDef::new("duration", "Duration", ColumnWidth::Fixed(80.0)),
        ColumnDef::new(
            "error",
            "Error",
            ColumnWidth::Flex {
                weight: 2.0,
                min: 120.0,
            },
        ),
    ]
}

impl PrometheusView {
    pub(super) fn render_targets(
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
                None => widgets::empty("Loading targets…", &colors),
            };
        };
        let filter = self.targets_tab.filter.read(cx).value().to_string();
        let health = self.targets_tab.health;
        self.targets_tab.rows = Arc::new(build_rows(
            &targets,
            &filter,
            health,
            &self.targets_tab.collapsed,
        ));
        let focused = self
            .targets_tab
            .filter
            .read(cx)
            .focus_handle(cx)
            .is_focused(window);
        let (up, down, unknown) = (
            targets.count(Health::Up),
            targets.count(Health::Down),
            targets.count(Health::Unknown),
        );
        let chip =
            |id: &'static str, label: String, value: Option<Health>, cx: &mut Context<Self>| {
                div()
                    .id(id)
                    .cursor_pointer()
                    .child(Chip::new(label).selected(health == value))
                    .on_click(cx.listener(move |this, _, _, cx| {
                        this.targets_tab.health = value;
                        cx.notify();
                    }))
            };
        let toolbar = widgets::toolbar(&colors)
            .child(div().child(format!("{} targets", targets.active.len())))
            .child(chip("targets-all", "All".into(), None, cx))
            .child(chip("targets-up", format!("Up {up}"), Some(Health::Up), cx))
            .child(chip(
                "targets-down",
                format!("Down {down}"),
                Some(Health::Down),
                cx,
            ))
            .when(unknown > 0, |this| {
                this.child(chip(
                    "targets-unknown",
                    format!("Unknown {unknown}"),
                    Some(Health::Unknown),
                    cx,
                ))
            })
            .child(div().flex_1())
            .child(widgets::filter_box(
                &self.targets_tab.filter,
                260.0,
                focused,
                &colors,
            ));
        let cols = columns();
        let body = if self.targets_tab.rows.is_empty() {
            widgets::empty(
                if targets.active.is_empty() {
                    "This server has no scrape targets."
                } else {
                    "No targets match."
                },
                &colors,
            )
        } else {
            v_flex()
                .flex_1()
                .min_h_0()
                .child(widgets::header(&cols, &colors))
                .child(
                    uniform_list(
                        "target-rows",
                        self.targets_tab.rows.len(),
                        cx.processor(|this, range: Range<usize>, _, cx| {
                            this.render_target_rows(range, cx)
                        }),
                    )
                    .flex_1()
                    .track_scroll(&self.targets_tab.scroll),
                )
                .into_any_element()
        };
        let details = self
            .selected_target()
            .map(|t| self.render_target_details(&t, cx));
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
                    .child(v_flex().flex_1().min_w_0().h_full().child(body))
                    .children(details),
            )
            .into_any_element()
    }

    fn selected_target(&self) -> Option<Target> {
        let (pool, url) = self.targets_tab.selected.as_ref()?;
        self.targets
            .data
            .as_ref()?
            .active
            .iter()
            .find(|t| &t.pool == pool && &t.url == url)
            .cloned()
    }

    fn render_target_rows(
        &mut self,
        range: Range<usize>,
        cx: &mut Context<Self>,
    ) -> Vec<AnyElement> {
        let colors = cx.colors().clone();
        let cols = columns();
        let now = Timestamp::now();
        let Some(targets) = self.targets.data.clone() else {
            return Vec::new();
        };
        let rows = self.targets_tab.rows.clone();
        range
            .filter_map(|index| {
                let row = rows.get(index)?;
                Some(match row {
                    Row::Pool(pool) => {
                        let collapsed = self.targets_tab.collapsed.contains(&pool.name);
                        let name = pool.name.clone();
                        let healthy = pool.down == 0 && pool.unknown == 0;
                        widgets::row(("pool", index), false, ROW_HEIGHT, &colors)
                            .bg(colors.subheader_background)
                            .cursor_pointer()
                            .gap(u(8.0))
                            .child(
                                Icon::new(if collapsed {
                                    IconName::ChevronRight
                                } else {
                                    IconName::ChevronDown
                                })
                                .size(12.0),
                            )
                            .child(
                                div()
                                    .font_weight(FontWeight::SEMIBOLD)
                                    .child(pool.name.clone()),
                            )
                            .child(
                                div()
                                    .text_size(u(11.5))
                                    .text_color(if healthy { colors.green } else { colors.red })
                                    .child(format!(
                                        "{}/{} up",
                                        pool.up,
                                        pool.up + pool.down + pool.unknown
                                    )),
                            )
                            .on_click(cx.listener(move |this, _, _, cx| {
                                let collapsed = &mut this.targets_tab.collapsed;
                                if !collapsed.remove(&name) {
                                    collapsed.insert(name.clone());
                                }
                                cx.notify();
                            }))
                            .into_any_element()
                    }
                    Row::Target(i) => {
                        let target = targets.active.get(*i)?;
                        let key = (target.pool.clone(), target.url.clone());
                        let selected = self.targets_tab.selected.as_ref() == Some(&key);
                        let labels = target
                            .labels
                            .iter()
                            .map(|(k, v)| format!("{k}=\"{v}\""))
                            .collect::<Vec<_>>()
                            .join(" ");
                        let cells: Vec<AnyElement> = vec![
                            widgets::health_label(target.health, &colors),
                            div()
                                .truncate()
                                .child(target.url.clone())
                                .into_any_element(),
                            div()
                                .truncate()
                                .text_color(colors.text_muted)
                                .child(labels)
                                .into_any_element(),
                            div()
                                .text_color(colors.text_muted)
                                .child(widgets::ago(target.last_scrape, now))
                                .into_any_element(),
                            div()
                                .text_color(colors.text_muted)
                                .child(widgets::seconds(target.scrape_duration))
                                .into_any_element(),
                            div()
                                .truncate()
                                .text_color(colors.red)
                                .child(target.last_error.clone().unwrap_or_default())
                                .into_any_element(),
                        ];
                        widgets::row(("target", index), selected, ROW_HEIGHT, &colors)
                            .children(cols.iter().zip(cells).map(|(def, cell)| {
                                widgets::column_cell(def)
                                    .when(def.mono, |d| {
                                        d.font_family(fonts::MONO).text_size(u(11.5))
                                    })
                                    .child(cell)
                            }))
                            .on_click(cx.listener(move |this, _, _, cx| {
                                this.targets_tab.selected = Some(key.clone());
                                cx.notify();
                            }))
                            .into_any_element()
                    }
                })
            })
            .collect()
    }

    fn render_target_details(&self, target: &Target, cx: &mut Context<Self>) -> AnyElement {
        let colors = cx.colors().clone();
        let now = Timestamp::now();
        let url = target.url.clone();
        let mut rows: Vec<(&'static str, AnyElement)> = vec![
            ("State", widgets::health_label(target.health, &colors)),
            ("Pool", div().child(target.pool.clone()).into_any_element()),
            (
                "Last scrape",
                div()
                    .child(match target.last_scrape {
                        Some(t) => format!(
                            "{} ({})",
                            widgets::ago(Some(t), now),
                            widgets::local_time(t)
                        ),
                        None => "never".into(),
                    })
                    .into_any_element(),
            ),
            (
                "Scrape duration",
                div()
                    .child(widgets::seconds(target.scrape_duration))
                    .into_any_element(),
            ),
        ];
        if let Some(instance) = target.labels.get("instance") {
            rows.insert(
                1,
                ("Instance", div().child(instance.clone()).into_any_element()),
            );
        }
        v_flex()
            .id("target-details")
            .flex_none()
            .w(u(380.0))
            .h_full()
            .overflow_y_scroll()
            .border_l_1()
            .border_color(colors.border_variant)
            .child(
                widgets::section("Endpoint", &colors)
                    .child(widgets::code(target.url.clone(), &colors))
                    .child(widgets::button(
                        "target-copy-url",
                        Some(IconName::Copy),
                        "Copy URL",
                        false,
                        &colors,
                        move |_, _, cx| widgets::copy(url.clone(), "the target URL", cx),
                    )),
            )
            .child(widgets::section("Status", &colors).child(widgets::kv(rows, &colors)))
            .when_some(target.last_error.clone(), |this, error| {
                this.child(
                    widgets::section("Last error", &colors)
                        .child(widgets::code(error, &colors).text_color(colors.red)),
                )
            })
            .child(
                widgets::section("Labels", &colors).child(widgets::chips(&target.labels, &colors)),
            )
            .child(
                widgets::section("Discovered labels (before relabeling)", &colors)
                    .child(widgets::chips(&target.discovered, &colors)),
            )
            .into_any_element()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::Labels;

    fn target(pool: &str, url: &str, health: Health, error: Option<&str>) -> Target {
        let mut labels = Labels::new();
        labels.insert("job".into(), pool.into());
        Target {
            pool: pool.into(),
            url: url.into(),
            health,
            labels,
            discovered: Labels::new(),
            last_error: error.map(str::to_string),
            last_scrape: None,
            scrape_duration: None,
        }
    }

    fn fixture() -> Targets {
        Targets {
            active: vec![
                target("a", "http://1/metrics", Health::Up, None),
                target(
                    "a",
                    "http://2/metrics",
                    Health::Down,
                    Some("connection refused"),
                ),
                target("b", "http://3/metrics", Health::Up, None),
            ],
            ..Default::default()
        }
    }

    #[test]
    fn a_header_per_pool() {
        let rows = build_rows(&fixture(), "", None, &HashSet::new());
        assert_eq!(rows.len(), 5);
        let Row::Pool(pool) = &rows[0] else {
            panic!("pool")
        };
        assert_eq!((pool.up, pool.down), (1, 1));
        assert_eq!(rows[1], Row::Target(0));
        assert!(matches!(rows[3], Row::Pool(_)));
    }

    #[test]
    fn collapsed_pools_hide_their_targets() {
        let collapsed: HashSet<String> = ["a".to_string()].into();
        let rows = build_rows(&fixture(), "", None, &collapsed);
        assert_eq!(rows.len(), 3);
    }

    #[test]
    fn filters_by_health_and_text() {
        let rows = build_rows(&fixture(), "", Some(Health::Down), &HashSet::new());
        assert_eq!(
            rows,
            [
                Row::Pool(Pool {
                    name: "a".into(),
                    up: 0,
                    down: 1,
                    unknown: 0,
                    dropped: 0
                }),
                Row::Target(1)
            ]
        );
        let rows = build_rows(&fixture(), "REFUSED", None, &HashSet::new());
        assert_eq!(rows.len(), 2, "matches the error, case-insensitively");
        assert!(build_rows(&fixture(), "nothing", None, &HashSet::new()).is_empty());
        let rows = build_rows(&fixture(), "job=b", None, &HashSet::new());
        assert_eq!(rows.len(), 2, "matches label pairs");
    }
}
