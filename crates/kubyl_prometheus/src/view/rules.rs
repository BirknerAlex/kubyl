//! Rule Health: alerting and recording rules by group with state, health, evaluation time and
//! the expression, like the Prometheus "Rules" page.

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
use kubyl_ui::{ActiveColors, Chip, Icon, IconName, fonts, h_flex, u, v_flex};

use super::{PrometheusView, widgets};
use crate::model::{Rule, RuleGroup, RuleKind};

const ROW_HEIGHT: f32 = 31.0;

#[derive(Clone, Debug, PartialEq)]
pub(crate) enum Row {
    /// An index into the groups.
    Group(usize),
    /// Group and rule index.
    Rule(usize, usize),
}

/// What identifies a rule across refreshes: rule names repeat across groups and within one (an
/// alert per severity), and indexes move when rules are added.
type RuleKey = String;

pub(super) fn rule_key(group: &RuleGroup, rule: &Rule) -> RuleKey {
    format!(
        "{}\0{}\0{}\0{}\0{:?}",
        group.file, group.name, rule.name, rule.query, rule.labels
    )
}

pub(crate) struct State {
    filter: Entity<InputState>,
    kind: Option<RuleKind>,
    problems_only: bool,
    collapsed: HashSet<(String, String)>,
    pub(super) selected: Option<RuleKey>,
    scroll: UniformListScrollHandle,
    rows: Arc<Vec<Row>>,
}

impl State {
    pub(crate) fn new(window: &mut Window, cx: &mut Context<PrometheusView>) -> Self {
        Self {
            filter: cx.new(|cx| {
                InputState::new(window, cx).placeholder("Filter by name, expression or label")
            }),
            kind: None,
            problems_only: false,
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

fn rule_matches(rule: &Rule, filter: &str) -> bool {
    let words: Vec<String> = filter.split_whitespace().map(str::to_lowercase).collect();
    if words.is_empty() {
        return true;
    }
    let mut haystack = format!("{} {}", rule.name, rule.query);
    for (k, v) in rule.labels.iter().chain(&rule.annotations) {
        haystack.push_str(&format!(" {k}={v}"));
    }
    let haystack = haystack.to_lowercase();
    words.iter().all(|w| haystack.contains(w))
}

/// Groups with at least one matching rule, each followed by its rules unless collapsed.
pub(crate) fn build_rows(
    groups: &[RuleGroup],
    filter: &str,
    kind: Option<RuleKind>,
    problems_only: bool,
    collapsed: &HashSet<(String, String)>,
) -> Vec<Row> {
    let mut rows = Vec::new();
    for (gi, group) in groups.iter().enumerate() {
        let shown: Vec<usize> = group
            .rules
            .iter()
            .enumerate()
            .filter(|(_, r)| {
                kind.is_none_or(|k| r.kind == k)
                    && (!problems_only || r.firing() || r.pending() || r.failing())
                    && rule_matches(r, filter)
            })
            .map(|(i, _)| i)
            .collect();
        if shown.is_empty() {
            continue;
        }
        rows.push(Row::Group(gi));
        if !collapsed.contains(&(group.file.clone(), group.name.clone())) {
            rows.extend(shown.into_iter().map(|ri| Row::Rule(gi, ri)));
        }
    }
    rows
}

fn columns() -> Vec<ColumnDef> {
    vec![
        ColumnDef::new("state", "State", ColumnWidth::Fixed(92.0)),
        ColumnDef::new(
            "rule",
            "Rule",
            ColumnWidth::Flex {
                weight: 3.0,
                min: 180.0,
            },
        )
        .mono(),
        ColumnDef::new("type", "Type", ColumnWidth::Fixed(84.0)),
        ColumnDef::new("health", "Health", ColumnWidth::Fixed(80.0)),
        ColumnDef::new("for", "For", ColumnWidth::Fixed(64.0)),
        ColumnDef::new("evaluated", "Last eval", ColumnWidth::Fixed(110.0)),
        ColumnDef::new("time", "Eval time", ColumnWidth::Fixed(84.0)),
    ]
}

fn state_cell(rule: &Rule, colors: &kubyl_ui::Colors) -> AnyElement {
    let (text, color) = match rule.state.as_deref() {
        Some("firing") => ("firing", colors.red),
        Some("pending") => ("pending", colors.yellow),
        Some(_) => ("inactive", colors.text_dim),
        None => {
            return div()
                .text_color(colors.text_dim)
                .child("—")
                .into_any_element();
        }
    };
    h_flex()
        .gap(u(6.0))
        .text_color(color)
        .child(kubyl_ui::StatusDot::new(color))
        .child(text)
        .into_any_element()
}

impl PrometheusView {
    pub(super) fn render_rules(
        &mut self,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let colors = cx.colors().clone();
        let Some(groups) = self.rules.data.clone() else {
            return match &self.rules.error {
                Some(error) => widgets::empty(format!("Couldn't read the rules: {error}"), &colors),
                None => widgets::empty("Loading rules…", &colors),
            };
        };
        let filter = self.rules_tab.filter.read(cx).value().to_string();
        let (kind, problems_only) = (self.rules_tab.kind, self.rules_tab.problems_only);
        self.rules_tab.rows = Arc::new(build_rows(
            &groups,
            &filter,
            kind,
            problems_only,
            &self.rules_tab.collapsed,
        ));
        let focused = self
            .rules_tab
            .filter
            .read(cx)
            .focus_handle(cx)
            .is_focused(window);
        let all: Vec<&Rule> = groups.iter().flat_map(|g| &g.rules).collect();
        let firing = all.iter().filter(|r| r.firing()).count();
        let pending = all.iter().filter(|r| r.pending()).count();
        let failing = all.iter().filter(|r| r.failing()).count();
        let kind_chip = |id: &'static str,
                         label: &'static str,
                         value: Option<RuleKind>,
                         cx: &mut Context<Self>| {
            div()
                .id(id)
                .cursor_pointer()
                .child(Chip::new(label).selected(kind == value))
                .on_click(cx.listener(move |this, _, _, cx| {
                    this.rules_tab.kind = value;
                    cx.notify();
                }))
        };
        let toolbar = widgets::toolbar(&colors)
            .child(div().child(format!("{} rules", all.len())))
            .when(firing > 0, |this| {
                this.child(
                    div()
                        .text_color(colors.red)
                        .child(format!("{firing} firing")),
                )
            })
            .when(pending > 0, |this| {
                this.child(
                    div()
                        .text_color(colors.yellow)
                        .child(format!("{pending} pending")),
                )
            })
            .when(failing > 0, |this| {
                this.child(
                    div()
                        .text_color(colors.red)
                        .child(format!("{failing} failing")),
                )
            })
            .child(kind_chip("rules-all", "All", None, cx))
            .child(kind_chip(
                "rules-alerting",
                "Alerting",
                Some(RuleKind::Alerting),
                cx,
            ))
            .child(kind_chip(
                "rules-recording",
                "Recording",
                Some(RuleKind::Recording),
                cx,
            ))
            .child(
                div()
                    .id("rules-problems")
                    .cursor_pointer()
                    .child(Chip::new("Firing, pending or failing").selected(problems_only))
                    .on_click(cx.listener(|this, _, _, cx| {
                        this.rules_tab.problems_only = !this.rules_tab.problems_only;
                        cx.notify();
                    })),
            )
            .child(div().flex_1())
            .child(widgets::filter_box(
                &self.rules_tab.filter,
                260.0,
                focused,
                &colors,
            ));
        let cols = columns();
        let body = if self.rules_tab.rows.is_empty() {
            widgets::empty(
                if all.is_empty() {
                    "This server has no rules."
                } else {
                    "No rules match."
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
                        "rule-rows",
                        self.rules_tab.rows.len(),
                        cx.processor(|this, range: Range<usize>, _, cx| {
                            this.render_rule_rows(range, cx)
                        }),
                    )
                    .flex_1()
                    .track_scroll(&self.rules_tab.scroll),
                )
                .into_any_element()
        };
        let details = self
            .selected_rule(&groups)
            .map(|(group, rule)| self.render_rule_details(group, rule, cx));
        v_flex()
            .size_full()
            .children(
                self.rules
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

    fn selected_rule<'a>(&self, groups: &'a [RuleGroup]) -> Option<(&'a RuleGroup, &'a Rule)> {
        let key = self.rules_tab.selected.as_ref()?;
        groups.iter().find_map(|group| {
            let rule = group.rules.iter().find(|r| &rule_key(group, r) == key)?;
            Some((group, rule))
        })
    }

    fn render_rule_rows(&mut self, range: Range<usize>, cx: &mut Context<Self>) -> Vec<AnyElement> {
        let colors = cx.colors().clone();
        let cols = columns();
        let now = Timestamp::now();
        let Some(groups) = self.rules.data.clone() else {
            return Vec::new();
        };
        let rows = self.rules_tab.rows.clone();
        range
            .filter_map(|index| {
                Some(match rows.get(index)? {
                    Row::Group(gi) => {
                        let group = groups.get(*gi)?;
                        let key = (group.file.clone(), group.name.clone());
                        let collapsed = self.rules_tab.collapsed.contains(&key);
                        let failing = group.rules.iter().filter(|r| r.failing()).count();
                        widgets::row(("rule-group", index), false, ROW_HEIGHT, &colors)
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
                                    .child(group.name.clone()),
                            )
                            .child(
                                div()
                                    .truncate()
                                    .text_size(u(11.5))
                                    .text_color(colors.text_dim)
                                    .child(format!(
                                        "{} · every {} · {} rules",
                                        group.file,
                                        widgets::short_duration(group.interval as i64),
                                        group.rules.len()
                                    )),
                            )
                            .when(failing > 0, |this| {
                                this.child(
                                    div()
                                        .text_size(u(11.5))
                                        .text_color(colors.red)
                                        .child(format!("{failing} failing")),
                                )
                            })
                            .on_click(cx.listener(move |this, _, _, cx| {
                                let collapsed = &mut this.rules_tab.collapsed;
                                if !collapsed.remove(&key) {
                                    collapsed.insert(key.clone());
                                }
                                cx.notify();
                            }))
                            .into_any_element()
                    }
                    Row::Rule(gi, ri) => {
                        let group = groups.get(*gi)?;
                        let rule = group.rules.get(*ri)?;
                        let key = rule_key(group, rule);
                        let selected = self.rules_tab.selected.as_ref() == Some(&key);
                        let cells: Vec<AnyElement> = vec![
                            state_cell(rule, &colors),
                            div().truncate().child(rule.name.clone()).into_any_element(),
                            div()
                                .text_color(colors.text_muted)
                                .child(match rule.kind {
                                    RuleKind::Alerting => "alerting",
                                    RuleKind::Recording => "recording",
                                })
                                .into_any_element(),
                            widgets::health_label(rule.health, &colors),
                            div()
                                .text_color(colors.text_muted)
                                .child(if rule.duration > 0.0 {
                                    widgets::short_duration(rule.duration as i64)
                                } else {
                                    String::new()
                                })
                                .into_any_element(),
                            div()
                                .text_color(colors.text_muted)
                                .child(widgets::ago(rule.last_evaluation, now))
                                .into_any_element(),
                            div()
                                .text_color(colors.text_muted)
                                .child(widgets::seconds(rule.evaluation_time))
                                .into_any_element(),
                        ];
                        widgets::row(("rule", index), selected, ROW_HEIGHT, &colors)
                            .children(cols.iter().zip(cells).map(|(def, cell)| {
                                widgets::column_cell(def)
                                    .when(def.mono, |d| {
                                        d.font_family(fonts::MONO).text_size(u(11.5))
                                    })
                                    .child(cell)
                            }))
                            .on_click(cx.listener(move |this, _, _, cx| {
                                this.rules_tab.selected = Some(key.clone());
                                cx.notify();
                            }))
                            .into_any_element()
                    }
                })
            })
            .collect()
    }

    fn render_rule_details(
        &self,
        group: &RuleGroup,
        rule: &Rule,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let colors = cx.colors().clone();
        let now = Timestamp::now();
        let query = rule.query.clone();
        let rows: Vec<(&'static str, AnyElement)> = vec![
            ("State", state_cell(rule, &colors)),
            ("Health", widgets::health_label(rule.health, &colors)),
            (
                "For",
                div()
                    .child(if rule.duration > 0.0 {
                        widgets::short_duration(rule.duration as i64)
                    } else {
                        "—".into()
                    })
                    .into_any_element(),
            ),
            ("Group", div().child(group.name.clone()).into_any_element()),
            ("File", div().child(group.file.clone()).into_any_element()),
            (
                "Interval",
                div()
                    .child(widgets::short_duration(group.interval as i64))
                    .into_any_element(),
            ),
            (
                "Last evaluation",
                div()
                    .child(match rule.last_evaluation {
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
                "Evaluation time",
                div()
                    .child(widgets::seconds(rule.evaluation_time))
                    .into_any_element(),
            ),
        ];
        v_flex()
            .id("rule-details")
            .flex_none()
            .w(u(400.0))
            .h_full()
            .overflow_y_scroll()
            .border_l_1()
            .border_color(colors.border_variant)
            .child(
                widgets::section(rule.name.clone(), &colors)
                    .child(widgets::code(rule.query.clone(), &colors))
                    .child(widgets::button(
                        "rule-copy-expression",
                        Some(IconName::Copy),
                        "Copy expression",
                        false,
                        &colors,
                        move |_, _, cx| widgets::copy(query.clone(), "the expression", cx),
                    )),
            )
            .child(widgets::section("Status", &colors).child(widgets::kv(rows, &colors)))
            .when_some(rule.last_error.clone(), |this, error| {
                this.child(
                    widgets::section("Last error", &colors)
                        .child(widgets::code(error, &colors).text_color(colors.red)),
                )
            })
            .when(!rule.labels.is_empty(), |this| {
                this.child(
                    widgets::section("Labels", &colors)
                        .child(widgets::chips(&rule.labels, &colors)),
                )
            })
            .when(!rule.annotations.is_empty(), |this| {
                this.child(widgets::section("Annotations", &colors).children(
                    rule.annotations.iter().map(|(k, v)| {
                        v_flex()
                            .gap(u(2.0))
                            .child(
                                div()
                                    .text_size(u(11.0))
                                    .text_color(colors.text_dim)
                                    .child(k.clone()),
                            )
                            .child(div().text_size(u(12.0)).child(v.clone()))
                    }),
                ))
            })
            .when(!rule.alerts.is_empty(), |this| {
                this.child(
                    widgets::section(format!("Active alerts ({})", rule.alerts.len()), &colors)
                        .children(rule.alerts.iter().take(50).map(|alert| {
                            v_flex()
                                .gap(u(4.0))
                                .child(
                                    h_flex()
                                        .gap(u(8.0))
                                        .text_size(u(11.5))
                                        .child(
                                            div()
                                                .text_color(if alert.state == "firing" {
                                                    colors.red
                                                } else {
                                                    colors.yellow
                                                })
                                                .child(alert.state.clone()),
                                        )
                                        .child(div().text_color(colors.text_dim).child(format!(
                                                "since {}",
                                                widgets::ago(alert.active_at, now)
                                                    .trim_end_matches(" ago")
                                            )))
                                        .when_some(alert.value.clone(), |this, value| {
                                            this.child(
                                                div()
                                                    .font_family(fonts::MONO)
                                                    .text_color(colors.text_muted)
                                                    .child(format!(
                                                        "value {}",
                                                        widgets::format_value(&value)
                                                    )),
                                            )
                                        }),
                                )
                                .child(widgets::chips(&alert.labels, &colors))
                        })),
                )
            })
            .into_any_element()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::{Health, Labels};

    fn rule(name: &str, kind: RuleKind, state: Option<&str>, health: Health) -> Rule {
        Rule {
            name: name.into(),
            kind,
            query: format!("{name}_expr > 1"),
            duration: 0.0,
            labels: Labels::new(),
            annotations: Labels::new(),
            health,
            last_error: None,
            evaluation_time: None,
            last_evaluation: None,
            state: state.map(str::to_string),
            alerts: Vec::new(),
        }
    }

    fn groups() -> Vec<RuleGroup> {
        let group = |name: &str, rules| RuleGroup {
            name: name.into(),
            file: "f.yaml".into(),
            interval: 30.0,
            evaluation_time: None,
            last_evaluation: None,
            rules,
        };
        vec![
            group(
                "a",
                vec![
                    rule("Quiet", RuleKind::Alerting, Some("inactive"), Health::Up),
                    rule("Loud", RuleKind::Alerting, Some("firing"), Health::Up),
                ],
            ),
            group(
                "b",
                vec![rule("job:x", RuleKind::Recording, None, Health::Down)],
            ),
        ]
    }

    #[test]
    fn groups_and_rules() {
        let rows = build_rows(&groups(), "", None, false, &HashSet::new());
        assert_eq!(rows.len(), 5);
        let collapsed: HashSet<_> = [("f.yaml".to_string(), "a".to_string())].into();
        assert_eq!(build_rows(&groups(), "", None, false, &collapsed).len(), 3);
    }

    #[test]
    fn filters() {
        let g = groups();
        let rows = build_rows(&g, "", Some(RuleKind::Recording), false, &HashSet::new());
        assert_eq!(rows, [Row::Group(1), Row::Rule(1, 0)]);
        let rows = build_rows(&g, "", None, true, &HashSet::new());
        assert_eq!(
            rows.len(),
            4,
            "the firing rule and the failing one, with their groups"
        );
        let rows = build_rows(&g, "loud_expr", None, false, &HashSet::new());
        assert_eq!(rows, [Row::Group(0), Row::Rule(0, 1)]);
    }
}
