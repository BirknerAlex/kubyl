//! The Rules tab: alerting rules by group with state, health, `for`, evaluation and the
//! expression; the rule's details; the `PrometheusRule` that defines it.

use std::collections::HashMap;
use std::ops::Range;

use gpui::{
    AnyElement, App, Context, Focusable as _, FontWeight, IntoElement, SharedString, Window, div,
    prelude::*, uniform_list,
};
use gpui_component::input::Input;
use jiff::Timestamp;
use kubyl_core::{ClusterId, ColumnDef, ColumnWidth, spawn_kube};
use kubyl_ui::{ActiveColors, Colors, Icon, IconName, fonts, h_flex, sizes, u, v_flex};
use serde_json::Value;

use super::{AlertsView, widgets};
use crate::model::Rule;

const ROW_HEIGHT: f32 = 31.0;

/// `PrometheusRule` objects of a cluster: `(namespace, name, [(group, [alert names])])`.
type RuleObjectList = Vec<(String, String, Vec<(String, Vec<String>)>)>;

#[derive(Clone)]
enum Loaded {
    Loading,
    Ready(std::sync::Arc<RuleObjectList>),
    Failed,
}

/// The `PrometheusRule` objects per cluster, read once when a rule's details need them.
#[derive(Default)]
pub(crate) struct RuleObjects(HashMap<ClusterId, Loaded>);

fn parse_rule_objects(list: &Value) -> RuleObjectList {
    list["items"]
        .as_array()
        .map(|items| {
            items
                .iter()
                .map(|item| {
                    let ns = item
                        .pointer("/metadata/namespace")
                        .and_then(Value::as_str)
                        .unwrap_or_default();
                    let name = item
                        .pointer("/metadata/name")
                        .and_then(Value::as_str)
                        .unwrap_or_default();
                    let groups = item
                        .pointer("/spec/groups")
                        .and_then(Value::as_array)
                        .map(|groups| {
                            groups
                                .iter()
                                .map(|g| {
                                    let alerts = g["rules"]
                                        .as_array()
                                        .map(|rules| {
                                            rules
                                                .iter()
                                                .filter_map(|r| {
                                                    r["alert"].as_str().map(str::to_string)
                                                })
                                                .collect()
                                        })
                                        .unwrap_or_default();
                                    (g["name"].as_str().unwrap_or_default().to_string(), alerts)
                                })
                                .collect()
                        })
                        .unwrap_or_default();
                    (ns.to_string(), name.to_string(), groups)
                })
                .collect()
        })
        .unwrap_or_default()
}

/// The object defining `alert` in `group` (any alert of the group when `alert` is empty).
fn find_in(objects: &RuleObjectList, group: &str, alert: &str) -> Option<(String, String)> {
    objects.iter().find_map(|(ns, name, groups)| {
        groups
            .iter()
            .any(|(g, alerts)| {
                g == group && (alert.is_empty() || alerts.iter().any(|a| a == alert))
            })
            .then(|| (ns.clone(), name.clone()))
    })
}

impl RuleObjects {
    /// The `PrometheusRule` defining `alert` in `group`; starts reading the list when needed.
    pub(crate) fn find(
        &mut self,
        cluster: &ClusterId,
        group: &str,
        alert: &str,
        cx: &mut Context<AlertsView>,
    ) -> Option<(String, String)> {
        match self.0.get(cluster) {
            Some(Loaded::Ready(objects)) => find_in(objects, group, alert),
            Some(Loaded::Loading | Loaded::Failed) => None,
            None => {
                self.load(cluster, cx);
                None
            }
        }
    }

    fn load(&mut self, cluster: &ClusterId, cx: &mut Context<AlertsView>) {
        let Some(client) =
            kubyl_kube::ConnectionManager::try_global(cx).and_then(|m| m.read(cx).client(cluster))
        else {
            return;
        };
        self.0.insert(cluster.clone(), Loaded::Loading);
        let task = spawn_kube(cx, async move {
            let request = http::Request::get("/apis/monitoring.coreos.com/v1/prometheusrules")
                .body(Vec::new())
                .ok()?;
            let list: Value = client.request(request).await.ok()?;
            Some(parse_rule_objects(&list))
        });
        let cluster = cluster.clone();
        cx.spawn(async move |this, cx| {
            let objects = task.await;
            this.update(cx, |this, cx| {
                this.rule_objects.0.insert(
                    cluster,
                    match objects {
                        Some(objects) => Loaded::Ready(std::sync::Arc::new(objects)),
                        None => Loaded::Failed,
                    },
                );
                cx.notify();
            })
            .ok();
        })
        .detach();
    }
}

/// A row of the Rules table.
#[derive(Clone)]
pub(crate) enum RuleRow {
    Group {
        cluster: ClusterId,
        name: String,
        collapsed: bool,
        count: usize,
        firing: usize,
        pending: usize,
        failing: usize,
    },
    Rule {
        cluster: ClusterId,
        rule: Box<Rule>,
    },
    /// The last error under a failing rule.
    Error(String),
}

fn rule_matches(rule: &Rule, filter: &str) -> bool {
    filter.is_empty()
        || rule.name.to_lowercase().contains(filter)
        || rule.group.to_lowercase().contains(filter)
        || rule.query.to_lowercase().contains(filter)
}

impl AlertsView {
    pub(crate) fn rule_rows(&self, cx: &App) -> Vec<RuleRow> {
        let filter = self.rules_filter.read(cx).value().trim().to_lowercase();
        let problems_only = self.options.rules_problems_only;
        let mut rows = Vec::new();
        for (cluster, groups) in self.rules(cx) {
            for group in groups.iter() {
                let rules: Vec<&Rule> = group
                    .rules
                    .iter()
                    .filter(|r| {
                        rule_matches(r, &filter) || group.name.to_lowercase().contains(&filter)
                    })
                    .filter(|r| !problems_only || r.problem())
                    .collect();
                if rules.is_empty() {
                    continue;
                }
                let collapsed = self.collapsed_rule_groups.contains(&group.name);
                rows.push(RuleRow::Group {
                    cluster: cluster.clone(),
                    name: group.name.clone(),
                    collapsed,
                    count: group.rules.len(),
                    firing: group.rules.iter().filter(|r| r.state == "firing").count(),
                    pending: group.rules.iter().filter(|r| r.state == "pending").count(),
                    failing: group.rules.iter().filter(|r| r.failing()).count(),
                });
                if collapsed {
                    continue;
                }
                for rule in rules {
                    rows.push(RuleRow::Rule {
                        cluster: cluster.clone(),
                        rule: Box::new(rule.clone()),
                    });
                    if let Some(error) = &rule.last_error {
                        rows.push(RuleRow::Error(error.clone()));
                    }
                }
            }
        }
        rows
    }

    pub(crate) fn move_rule(&mut self, delta: isize, cx: &mut Context<Self>) {
        let rows = self.rule_rows(cx);
        let keys: Vec<(usize, (String, String))> = rows
            .iter()
            .enumerate()
            .filter_map(|(i, r)| match r {
                RuleRow::Rule { rule, .. } => Some((i, (rule.group.clone(), rule.name.clone()))),
                _ => None,
            })
            .collect();
        if keys.is_empty() {
            return;
        }
        let current = self
            .selected_rule
            .as_ref()
            .and_then(|sel| keys.iter().position(|(_, k)| k == sel));
        let next = match current {
            None => 0,
            Some(pos) => (pos as isize + delta).clamp(0, keys.len() as isize - 1) as usize,
        };
        self.selected_rule = Some(keys[next].1.clone());
        self.rules_scroll
            .scroll_to_item(keys[next].0, gpui::ScrollStrategy::Nearest);
        cx.notify();
    }

    fn rule_columns(&self) -> Vec<ColumnDef> {
        let flex = |weight: f32, min: f32| ColumnWidth::Flex { weight, min };
        vec![
            ColumnDef::new("state", "State", ColumnWidth::Fixed(84.0)),
            ColumnDef::new("rule", "Alert rule", flex(1.2, 160.0)),
            ColumnDef::new("health", "Health", ColumnWidth::Fixed(76.0)),
            ColumnDef::new("for", "For", ColumnWidth::Fixed(48.0)),
            ColumnDef::new("evaluated", "Evaluated", ColumnWidth::Fixed(92.0)),
            ColumnDef::new("expression", "Expression", flex(2.0, 160.0)),
        ]
    }

    pub(crate) fn render_rules_tab(
        &mut self,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let colors = cx.colors().clone();
        if let Some(blocking) = self.blocking_state(cx) {
            return self.render_blocking(blocking, cx);
        }
        let has_rules = self
            .clusters(cx)
            .iter()
            .any(|c| self.state(c, cx).is_some_and(|s| s.has_rules()));
        if !has_rules {
            return self.focus_area().child(widgets::empty(
                "Rules come from Prometheus' rules API. No Prometheus was found for this cluster (phase 07's discovery), or alerts.clusters.<cluster>.rules is off.",
                &colors,
            )).into_any_element();
        }
        let mut total = 0;
        let mut firing = 0;
        let mut pending = 0;
        let mut failing = 0;
        let mut errors: Vec<String> = Vec::new();
        for (cluster, groups) in self.rules(cx) {
            for rule in groups.iter().flat_map(|g| &g.rules) {
                total += 1;
                firing += usize::from(rule.state == "firing");
                pending += usize::from(rule.state == "pending");
                failing += usize::from(rule.failing());
            }
            if let Some(error) = self.state(&cluster, cx).and_then(|s| s.rules_error.clone()) {
                errors.push(error.to_string());
            }
        }
        let focused = self
            .rules_filter
            .read(cx)
            .focus_handle(cx)
            .is_focused(window);
        let problems_only = self.options.rules_problems_only;
        let toolbar = h_flex()
            .flex_none()
            .h(u(38.0))
            .px(u(14.0))
            .gap(u(6.0))
            .border_b_1()
            .border_color(colors.border_variant)
            .text_size(u(12.5))
            .child(
                div()
                    .font_weight(FontWeight::SEMIBOLD)
                    .child(format!("{total} rules")),
            )
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
                        .child(format!("· {pending} pending")),
                )
            })
            .when(failing > 0, |this| {
                this.child(
                    div()
                        .text_color(colors.red)
                        .child(format!("· {failing} failing")),
                )
            })
            .when(!errors.is_empty(), |this| {
                this.child(
                    div()
                        .truncate()
                        .text_color(colors.yellow)
                        .child(format!("· rules are stale: {}", errors.join("; "))),
                )
            })
            .child(div().flex_1())
            .child(
                div()
                    .id("rules-problems-only")
                    .cursor_pointer()
                    .child(
                        kubyl_ui::Chip::new("Only firing, pending or failing")
                            .selected(problems_only),
                    )
                    .on_click(cx.listener(|this, _, _, cx| {
                        this.options.rules_problems_only = !this.options.rules_problems_only;
                        this.save_options(cx);
                        cx.notify();
                    })),
            )
            .child(
                div()
                    .flex_none()
                    .w(u(200.0))
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
                    .child(Icon::new(IconName::Funnel).size(12.0))
                    .child(
                        div().flex_1().min_w_0().child(
                            Input::new(&self.rules_filter)
                                .appearance(false)
                                .text_size(u(12.0)),
                        ),
                    ),
            );
        let rows = self.rule_rows(cx);
        let columns = self.rule_columns();
        let body = if rows.is_empty() {
            widgets::empty("No rules match.", &colors)
        } else {
            v_flex()
                .flex_1()
                .min_h_0()
                .child(widgets::header(&columns, &colors))
                .child(
                    uniform_list(
                        "rule-rows",
                        rows.len(),
                        cx.processor(|this, range: Range<usize>, _, cx| {
                            let rows = this.rule_rows(cx);
                            this.render_rule_rows(&rows, range, cx)
                        }),
                    )
                    .flex_1()
                    .track_scroll(&self.rules_scroll),
                )
                .into_any_element()
        };
        let details = self
            .rule_details_open
            .then(|| self.selected_rule(cx))
            .flatten()
            .map(|(cluster, rule)| self.render_rule_details(&cluster, &rule, cx));
        v_flex()
            .size_full()
            .child(toolbar)
            .child(
                self.focus_area()
                    .items_start()
                    .child(v_flex().flex_1().min_w_0().h_full().child(body))
                    .children(details),
            )
            .into_any_element()
    }

    fn render_rule_rows(
        &mut self,
        rows: &[RuleRow],
        range: Range<usize>,
        cx: &mut Context<Self>,
    ) -> Vec<AnyElement> {
        let colors = cx.colors().clone();
        let columns = self.rule_columns();
        let now = Timestamp::now();
        range
            .filter_map(|index| {
                Some(match rows.get(index)?.clone() {
                    RuleRow::Group {
                        cluster,
                        name,
                        collapsed,
                        count,
                        firing,
                        pending,
                        failing,
                    } => {
                        let object = self.rule_objects.find(&cluster, &name, "", cx);
                        let mut meta = format!("{count} rule{}", if count == 1 { "" } else { "s" });
                        if firing > 0 {
                            meta.push_str(&format!(" · {firing} firing"));
                        }
                        if pending > 0 {
                            meta.push_str(&format!(" · {pending} pending"));
                        }
                        if failing > 0 {
                            meta.push_str(&format!(" · {failing} failing"));
                        }
                        let toggle = name.clone();
                        widgets::row(("rule-group", index), false, ROW_HEIGHT, &colors)
                            .gap(u(8.0))
                            .cursor_pointer()
                            .on_click(cx.listener(move |this, _, _, cx| {
                                if !this.collapsed_rule_groups.remove(&toggle) {
                                    this.collapsed_rule_groups.insert(toggle.clone());
                                }
                                cx.notify();
                            }))
                            .child(
                                Icon::new(if collapsed {
                                    IconName::ChevronRight
                                } else {
                                    IconName::ChevronDown
                                })
                                .size(12.0)
                                .color(colors.text_dim),
                            )
                            .child(div().font_weight(FontWeight::SEMIBOLD).child(name.clone()))
                            .when(self.cluster.is_none(), |this| {
                                this.child(
                                    div()
                                        .text_color(colors.text_dim)
                                        .child(Self::cluster_name(&cluster, cx)),
                                )
                            })
                            .when_some(object, |this, (ns, obj)| {
                                let open = (ns.clone(), obj.clone());
                                let cluster = cluster.clone();
                                this.child(div().text_color(colors.text_faint).child("·"))
                                    .child(
                                        widgets::link(
                                            SharedString::from(format!("rule-object-{index}")),
                                            format!("{ns}/{obj}"),
                                            &colors,
                                            move |_, window, cx| {
                                                crate::actions::edit_prometheus_rule(
                                                    &cluster, &open.0, &open.1, window, cx,
                                                )
                                            },
                                        )
                                        .font_family(fonts::MONO)
                                        .text_size(u(11.5)),
                                    )
                            })
                            .child(div().text_color(colors.text_dim).child(format!("· {meta}")))
                            .into_any_element()
                    }
                    RuleRow::Error(error) => {
                        widgets::row(("rule-error", index), false, ROW_HEIGHT, &colors)
                            .pl(u(96.0))
                            .gap(u(6.0))
                            .text_color(colors.red)
                            .font_family(fonts::MONO)
                            .text_size(u(11.5))
                            .child(
                                Icon::new(IconName::TriangleAlert)
                                    .size(12.0)
                                    .color(colors.red),
                            )
                            .child(div().truncate().child(error))
                            .into_any_element()
                    }
                    RuleRow::Rule { cluster, rule } => {
                        let key = (rule.group.clone(), rule.name.clone());
                        let selected = self.selected_rule.as_ref() == Some(&key);
                        let _ = cluster;
                        widgets::row(("rule-row", index), selected, ROW_HEIGHT, &colors)
                            .children(columns.iter().map(|def| {
                                widgets::column_cell(def).child(rule_cell(
                                    &rule,
                                    def.id.as_ref(),
                                    now,
                                    &colors,
                                ))
                            }))
                            .on_click(cx.listener(move |this, _, window, cx| {
                                this.focus.focus(window, cx);
                                this.selected_rule = Some(key.clone());
                                this.rule_details_open = true;
                                cx.notify();
                            }))
                            .into_any_element()
                    }
                })
            })
            .collect()
    }

    fn render_rule_details(
        &mut self,
        cluster: &ClusterId,
        rule: &Rule,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let colors = cx.colors().clone();
        let health = if rule.failing() {
            ("fails to evaluate", colors.red, IconName::CircleX)
        } else {
            ("healthy", colors.green, IconName::CircleCheck)
        };
        let rule_section = self.render_rule_section(cluster, rule, cx);
        let labels = (!rule.labels.is_empty()).then(|| {
            widgets::section("Labels", &colors).child(
                h_flex().flex_wrap().gap(u(5.0)).children(
                    rule.labels
                        .iter()
                        .map(|(k, v)| widgets::label_chip(k, v, &colors)),
                ),
            )
        });
        let annotations = (!rule.annotations.is_empty()).then(|| {
            widgets::section("Annotations", &colors).child(
                v_flex()
                    .gap(u(4.0))
                    .text_size(u(12.0))
                    .children(rule.annotations.iter().map(|(k, v)| {
                        h_flex()
                            .items_start()
                            .gap(u(8.0))
                            .child(
                                div()
                                    .flex_none()
                                    .w(u(96.0))
                                    .text_color(colors.text_dim)
                                    .child(k.clone()),
                            )
                            .child(
                                div()
                                    .flex_1()
                                    .min_w_0()
                                    .whitespace_normal()
                                    .child(v.clone()),
                            )
                    })),
            )
        });
        let show = rule.name.clone();
        let close = cx.listener(|this, _, _, cx| {
            this.rule_details_open = false;
            cx.notify();
        });
        v_flex()
            .flex_none()
            .w(u(330.0))
            .h_full()
            .border_l_1()
            .border_color(colors.border)
            .bg(colors.panel)
            .child(
                h_flex()
                    .flex_none()
                    .h(u(34.0))
                    .px(u(14.0))
                    .border_b_1()
                    .border_color(colors.border_variant)
                    .child(
                        div()
                            .flex_1()
                            .font_weight(FontWeight::SEMIBOLD)
                            .child("Alerting rule"),
                    )
                    .child(
                        kubyl_ui::IconButton::new("rule-details-close", IconName::X)
                            .icon_size(12.0)
                            .on_click(close),
                    ),
            )
            .child(
                v_flex()
                    .id("rule-details-scroll")
                    .flex_1()
                    .min_h_0()
                    .overflow_y_scroll()
                    .child(
                        v_flex()
                            .px(u(14.0))
                            .py(u(12.0))
                            .gap(u(6.0))
                            .border_b_1()
                            .border_color(colors.border_variant)
                            .child(
                                div()
                                    .font_family(fonts::MONO)
                                    .text_size(u(13.5))
                                    .font_weight(FontWeight::SEMIBOLD)
                                    .child(rule.name.clone()),
                            )
                            .child(
                                h_flex()
                                    .gap(u(10.0))
                                    .text_size(u(12.0))
                                    .child(
                                        h_flex()
                                            .gap(u(5.0))
                                            .text_color(health.1)
                                            .child(Icon::new(health.2).size(12.0).color(health.1))
                                            .child(health.0),
                                    )
                                    .child(
                                        div()
                                            .text_color(colors.text_dim)
                                            .child(format!("group {}", rule.group)),
                                    ),
                            )
                            .child(widgets::text_button(
                                "rule-show-alerts",
                                "Show its alerts",
                                &colors,
                                {
                                    let weak = cx.entity().downgrade();
                                    move |_, window, cx| {
                                        let show = show.clone();
                                        weak.update(cx, |this, cx| {
                                            this.show_rule_alerts(&show, window, cx)
                                        })
                                        .ok();
                                    }
                                },
                            )),
                    )
                    .child(rule_section)
                    .children(labels)
                    .children(annotations),
            )
            .into_any_element()
    }

    /// The Alerts tab filtered to a rule's alerts.
    pub(crate) fn show_rule_alerts(
        &mut self,
        name: &str,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let text =
            crate::matchers::Matcher::new("alertname", crate::matchers::MatchOp::Equal, name)
                .to_string();
        self.filter_input
            .update(cx, |input, cx| input.set_value(text, window, cx));
        self.filters.show_suppressed = true;
        self.tab = super::Tab::Alerts;
        self.rebuild(cx);
    }
}

fn rule_cell(rule: &Rule, column: &str, now: Timestamp, colors: &Colors) -> AnyElement {
    match column {
        "state" => {
            let (label, color) = match rule.state.as_str() {
                "firing" => ("firing", colors.red),
                "pending" => ("pending", colors.yellow),
                _ => ("OK", colors.green),
            };
            h_flex()
                .gap(u(6.0))
                .text_color(color)
                .child(kubyl_ui::StatusDot::new(color))
                .child(label)
                .into_any_element()
        }
        "rule" => div()
            .truncate()
            .font_family(fonts::MONO)
            .text_size(u(12.0))
            .child(rule.name.clone())
            .into_any_element(),
        "health" => {
            let (label, color, icon) = if rule.failing() {
                ("error", colors.red, IconName::CircleX)
            } else if rule.health == "ok" {
                ("ok", colors.green, IconName::CircleCheck)
            } else {
                ("unknown", colors.text_dim, IconName::Minus)
            };
            h_flex()
                .gap(u(5.0))
                .text_color(color)
                .child(Icon::new(icon).size(12.0).color(color))
                .child(label)
                .into_any_element()
        }
        "for" => div()
            .font_family(fonts::MONO)
            .text_size(u(11.5))
            .child(if rule.duration > 0.0 {
                widgets::short_duration(rule.duration as i64)
            } else {
                "—".into()
            })
            .into_any_element(),
        "evaluated" => div()
            .font_family(fonts::MONO)
            .text_size(u(11.5))
            .text_color(colors.text_muted)
            .child(match rule.last_evaluation {
                Some(at) => format!(
                    "{} · {}",
                    widgets::ago(Some(at), now),
                    super::details::eval_time(rule.evaluation_time)
                ),
                None => "—".into(),
            })
            .into_any_element(),
        "expression" => div()
            .truncate()
            .font_family(fonts::MONO)
            .text_size(u(11.5))
            .text_color(colors.text_muted)
            .child(rule.query.replace('\n', " "))
            .into_any_element(),
        _ => div().into_any_element(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn rule_objects_by_group_and_alert() {
        let objects = parse_rule_objects(&json!({"items": [
            {"metadata": {"namespace": "payments", "name": "payments-slo"},
             "spec": {"groups": [{"name": "payments.rules", "rules": [
                {"alert": "CheckoutErrorBudgetBurn", "expr": "x"},
                {"record": "job:rate5m", "expr": "y"}
             ]}]}},
            {"metadata": {"namespace": "monitoring", "name": "kube-apps"},
             "spec": {"groups": [{"name": "kubernetes-apps", "rules": [{"alert": "KubePodCrashLooping"}]}]}}
        ]}));
        assert_eq!(
            find_in(&objects, "payments.rules", "CheckoutErrorBudgetBurn"),
            Some(("payments".into(), "payments-slo".into()))
        );
        assert_eq!(
            find_in(&objects, "kubernetes-apps", ""),
            Some(("monitoring".into(), "kube-apps".into()))
        );
        assert_eq!(find_in(&objects, "payments.rules", "Other"), None);
    }
}
