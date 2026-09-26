//! Install plans: waiting for approval first, then installing and failed, then complete.

use std::ops::Range;
use std::sync::Arc;

use gpui::{
    AnyElement, App, Context, FontWeight, IntoElement, Window, div, prelude::*, uniform_list,
};
use kubyl_core::actions::OpenView;
use kubyl_core::{ColumnDef, ColumnWidth, ResourceRef, Tone, ViewKind, ViewRequest};
use kubyl_ui::{ActiveColors, Button, Colors, IconName, fonts, h_flex, u, v_flex};

use super::{OperatorsView, SubTab};
use crate::olm::model::InstallPlan;
use crate::service::Olm;
use crate::widgets;

const ROW_HEIGHT: f32 = 32.0;

/// A row of the list: a group header or a plan.
#[derive(Clone, Debug)]
pub enum PlanRow {
    Group(String),
    Plan(Arc<InstallPlan>),
}

pub fn plan_key(plan: &InstallPlan) -> String {
    format!("plan:{}/{}", plan.namespace, plan.name)
}

fn phase_tone(plan: &InstallPlan) -> Tone {
    match plan.phase.as_str() {
        "RequiresApproval" if !plan.approved => Tone::Warning,
        "Installing" | "Planning" | "RequiresApproval" => Tone::Info,
        "Failed" => Tone::Bad,
        "Complete" => Tone::Good,
        _ => Tone::Muted,
    }
}

/// Groups plans: waiting for approval, installing and failed, complete.
pub fn rows(plans: &[Arc<InstallPlan>]) -> Vec<PlanRow> {
    let waiting: Vec<&Arc<InstallPlan>> = plans.iter().filter(|p| p.needs_approval()).collect();
    let open: Vec<&Arc<InstallPlan>> = plans
        .iter()
        .filter(|p| !p.needs_approval() && !p.complete())
        .collect();
    let done: Vec<&Arc<InstallPlan>> = plans.iter().filter(|p| p.complete()).collect();
    let mut out = Vec::new();
    for (label, group) in [
        ("Waiting for approval", waiting),
        ("Installing and failed", open),
        ("Complete", done),
    ] {
        if group.is_empty() {
            continue;
        }
        out.push(PlanRow::Group(format!("{label} · {}", group.len())));
        out.extend(group.into_iter().map(|p| PlanRow::Plan(p.clone())));
    }
    out
}

impl OperatorsView {
    pub(crate) fn selected_plan(&self, cx: &App) -> Option<Arc<InstallPlan>> {
        let key = self.selected.get(&SubTab::Plans)?;
        self.snapshot(cx)?
            .plans
            .iter()
            .find(|p| &plan_key(p) == key)
            .cloned()
    }

    fn plan_columns(&self) -> Vec<ColumnDef> {
        vec![
            ColumnDef::new("name", "Name", ColumnWidth::Fixed(140.0)),
            ColumnDef::new("namespace", "Namespace", ColumnWidth::Fixed(120.0)),
            ColumnDef::new(
                "csvs",
                "Cluster service versions",
                ColumnWidth::Flex {
                    weight: 1.4,
                    min: 160.0,
                },
            ),
            ColumnDef::new("approval", "Approval", ColumnWidth::Fixed(90.0)),
            ColumnDef::new("phase", "Phase", ColumnWidth::Fixed(150.0)),
            ColumnDef::new("age", "Age", ColumnWidth::Fixed(60.0)),
        ]
    }

    pub(crate) fn render_plans(&mut self, _: &mut Window, cx: &mut Context<Self>) -> AnyElement {
        let colors = cx.colors().clone();
        let Some(snapshot) = self.snapshot(cx) else {
            return widgets::empty("Loading…", &colors);
        };
        let query = self.query(cx);
        let plans: Vec<Arc<InstallPlan>> = snapshot
            .plans
            .iter()
            .filter(|p| {
                query.is_empty()
                    || Self::matches(
                        &query,
                        &format!(
                            "{} {} {} {}",
                            p.name,
                            p.namespace,
                            p.csv_names.join(" "),
                            p.phase
                        ),
                    )
            })
            .cloned()
            .collect();
        self.plan_rows = rows(&plans);
        self.keys = self
            .plan_rows
            .iter()
            .filter_map(|r| match r {
                PlanRow::Plan(p) => Some(plan_key(p)),
                PlanRow::Group(_) => None,
            })
            .collect();
        if self.selected_key().is_none()
            && let Some(first) = self.keys.first().cloned()
        {
            self.selected.insert(SubTab::Plans, first);
        }
        let body = if self.plan_rows.is_empty() {
            widgets::empty(
                if snapshot.plans.is_empty() {
                    "No install plans."
                } else {
                    "No install plans match the filter."
                },
                &colors,
            )
        } else {
            let columns = self.plan_columns();
            v_flex()
                .flex_1()
                .min_h_0()
                .child(widgets::header(&columns, &colors))
                .child(
                    uniform_list(
                        "plan-rows",
                        self.plan_rows.len(),
                        cx.processor(|this, range: Range<usize>, _, cx| {
                            this.render_plan_rows(range, cx)
                        }),
                    )
                    .flex_1()
                    .track_scroll(&self.scroll),
                )
                .into_any_element()
        };
        let details = self
            .details_open
            .then(|| self.selected_plan(cx))
            .flatten()
            .map(|plan| self.render_plan_details(plan, cx));
        self.focus_area()
            .items_start()
            .child(v_flex().flex_1().min_w_0().h_full().child(body))
            .children(details)
            .into_any_element()
    }

    fn render_plan_rows(&mut self, range: Range<usize>, cx: &mut Context<Self>) -> Vec<AnyElement> {
        let colors = cx.colors().clone();
        let columns = self.plan_columns();
        let selected = self.selected_key().cloned();
        range
            .filter_map(|index| {
                let row = self.plan_rows.get(index)?.clone();
                Some(match row {
                    PlanRow::Group(label) => {
                        widgets::group_row(("plan-group", index), label, &colors)
                    }
                    PlanRow::Plan(plan) => {
                        let key = plan_key(&plan);
                        widgets::row(
                            ("plan-row", index),
                            selected.as_ref() == Some(&key),
                            ROW_HEIGHT,
                            &colors,
                        )
                        .children(columns.iter().map(|def| {
                            widgets::column_cell(def).child(plan_cell(
                                &plan,
                                def.id.as_ref(),
                                &colors,
                            ))
                        }))
                        .on_click(
                            cx.listener(move |this, event: &gpui::ClickEvent, window, cx| {
                                this.focus.focus(window, cx);
                                this.select(key.clone(), cx);
                                if event.click_count() == 2 {
                                    this.details_open = true;
                                }
                            }),
                        )
                        .into_any_element()
                    }
                })
            })
            .collect()
    }

    fn render_plan_details(
        &mut self,
        plan: Arc<InstallPlan>,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let colors = cx.colors().clone();
        let read_only = self.read_only(cx);
        let snapshot = self.snapshot(cx);
        let operator = snapshot.as_ref().and_then(|s| {
            s.operators
                .iter()
                .find(|o| {
                    o.subscription.as_ref().is_some_and(|sub| {
                        sub.namespace == plan.namespace && plan.subscriptions.contains(&sub.name)
                    })
                })
                .cloned()
        });
        let label = if plan.needs_approval() {
            if operator.as_ref().is_some_and(|o| o.csv.is_some()) {
                "Upgrade available"
            } else {
                "Approval required"
            }
        } else {
            plan.phase.as_str()
        };
        let cluster = self.cluster.clone();
        let mut rows = Vec::new();
        if let Some(op) = &operator {
            let target = plan
                .csv_names
                .iter()
                .find_map(|n| plan.version_of(n))
                .unwrap_or_default();
            rows.push((
                "Operator",
                h_flex()
                    .gap(u(6.0))
                    .child(op.display_name())
                    .child(
                        div()
                            .font_family(fonts::MONO)
                            .text_size(u(11.5))
                            .text_color(colors.text_dim)
                            .child(format!("{} → {target}", op.version().unwrap_or("—"))),
                    )
                    .into_any_element(),
            ));
        }
        if let Some(sub) = plan.subscriptions.first() {
            let key = format!("sub:{}/{sub}", plan.namespace);
            rows.push((
                "Subscription",
                widgets::link(
                    "plan-subscription",
                    format!("{}/{sub}", plan.namespace),
                    &colors,
                    {
                        let weak = cx.entity().downgrade();
                        move |_, window, cx| {
                            let key = key.clone();
                            weak.update(cx, |this, cx| {
                                this.set_tab(SubTab::Subscriptions, window, cx);
                                this.selected.insert(SubTab::Subscriptions, key);
                                cx.notify();
                            })
                            .ok();
                        }
                    },
                )
                .into_any_element(),
            ));
        }
        rows.push((
            "Approval",
            widgets::text(format!(
                "{}{}",
                plan.approval.label(),
                if plan.approved { " · approved" } else { "" }
            )),
        ));
        rows.push((
            "Created",
            widgets::text(format!("{} ago", widgets::ago(plan.created))),
        ));
        let yaml_ref = ResourceRef::object(
            cluster.clone(),
            crate::olm::model::install_plans(),
            Some(plan.namespace.clone()),
            plan.name.clone(),
        );
        let busy =
            Olm::global(cx).is_some_and(|o| o.read(cx).is_busy(&format!("plan:{}", plan.key())));
        let (approve_plan, review_plan) = (plan.clone(), plan.clone());
        let (approve_cluster, review_cluster) = (cluster.clone(), cluster.clone());
        let steps = plan.steps.len();
        let mut steps_section = widgets::section(
            if steps == 0 {
                "Steps".to_string()
            } else {
                format!("Steps · {steps} resources")
            },
            &colors,
        );
        if plan.unpacking {
            steps_section = steps_section.child(
                div()
                    .text_size(u(12.0))
                    .text_color(colors.text_dim)
                    .child("OLM is unpacking the bundle…"),
            );
        }
        for step in plan.steps.iter().take(30) {
            let action = match step.status.as_str() {
                "Unknown" | "NotPresent" => ("create", colors.green),
                "Present" => ("update", colors.accent),
                "Created" => ("created", colors.text_dim),
                other => (other, colors.text_dim),
            };
            steps_section = steps_section.child(
                h_flex()
                    .gap(u(6.0))
                    .py(u(2.0))
                    .border_b_1()
                    .border_color(colors.border_variant)
                    .child(
                        v_flex()
                            .flex_1()
                            .min_w_0()
                            .child(
                                div()
                                    .text_size(u(11.0))
                                    .text_color(colors.text_dim)
                                    .child(step.kind.clone()),
                            )
                            .child(
                                div()
                                    .truncate()
                                    .font_family(fonts::MONO)
                                    .text_size(u(11.5))
                                    .child(step.name.clone()),
                            ),
                    )
                    .child(
                        div()
                            .flex_none()
                            .text_size(u(11.5))
                            .text_color(action.1)
                            .child(action.0.to_string()),
                    ),
            );
        }
        if steps > 30 {
            steps_section = steps_section.child(
                div()
                    .text_size(u(11.5))
                    .text_color(colors.text_dim)
                    .child(format!("and {} more", steps - 30)),
            );
        }
        if let Some(failure) = plan.failure() {
            steps_section = steps_section.child(widgets::note(
                IconName::CircleX,
                colors.red,
                failure,
                &colors,
            ));
        }
        v_flex()
            .flex_none()
            .w(u(340.0))
            .h_full()
            .bg(colors.panel)
            .border_l_1()
            .border_color(colors.border)
            .child(
                h_flex()
                    .flex_none()
                    .h(u(36.0))
                    .px(u(12.0))
                    .gap(u(8.0))
                    .border_b_1()
                    .border_color(colors.border_variant)
                    .child(
                        div()
                            .flex_1()
                            .truncate()
                            .font_family(fonts::MONO)
                            .font_weight(FontWeight::MEDIUM)
                            .child(plan.name.clone()),
                    )
                    .child(
                        div()
                            .text_size(u(11.5))
                            .text_color(colors.text_dim)
                            .child(plan.namespace.clone()),
                    )
                    .child(
                        kubyl_ui::IconButton::new("plan-details-close", IconName::X)
                            .icon_size(13.0)
                            .on_click(cx.listener(|this, _, _, cx| {
                                this.details_open = false;
                                cx.notify();
                            })),
                    ),
            )
            .child(
                v_flex()
                    .id("plan-details")
                    .flex_1()
                    .min_h_0()
                    .overflow_y_scroll()
                    .child(
                        widgets::plain_section(&colors)
                            .child(widgets::pill(label.to_string(), phase_tone(&plan), &colors))
                            .child(widgets::kv(rows, &colors))
                            .child(
                                h_flex()
                                    .pt(u(4.0))
                                    .gap(u(6.0))
                                    .when(plan.needs_approval() && !read_only, |this| {
                                        this.child(
                                            Button::new("plan-approve")
                                                .primary()
                                                .icon(IconName::Check)
                                                .label(if busy {
                                                    "Approving…"
                                                } else {
                                                    "Approve…"
                                                })
                                                .disabled(busy)
                                                .on_click(move |_, window, cx| {
                                                    crate::dialogs::open_review(
                                                        approve_cluster.clone(),
                                                        approve_plan.clone(),
                                                        window,
                                                        cx,
                                                    )
                                                }),
                                        )
                                    })
                                    .when(plan.needs_approval(), |this| {
                                        this.child(
                                            Button::new("plan-review")
                                                .icon(IconName::Diff)
                                                .label("Review changes")
                                                .on_click(move |_, window, cx| {
                                                    crate::dialogs::open_review(
                                                        review_cluster.clone(),
                                                        review_plan.clone(),
                                                        window,
                                                        cx,
                                                    )
                                                }),
                                        )
                                    })
                                    .child(
                                        Button::new("plan-yaml")
                                            .ghost()
                                            .icon(IconName::Code)
                                            .label("YAML")
                                            .on_click(move |_, window, cx| {
                                                window.dispatch_action(
                                                    Box::new(OpenView(ViewRequest::for_resource(
                                                        ViewKind::Yaml,
                                                        yaml_ref.clone(),
                                                    ))),
                                                    cx,
                                                )
                                            }),
                                    ),
                            ),
                    )
                    .child(steps_section),
            )
            .into_any_element()
    }
}

fn plan_cell(plan: &InstallPlan, column: &str, colors: &Colors) -> AnyElement {
    match column {
        "name" => widgets::mono(plan.name.clone()),
        "namespace" => div()
            .truncate()
            .font_family(fonts::MONO)
            .text_size(u(12.0))
            .text_color(colors.text_muted)
            .child(plan.namespace.clone())
            .into_any_element(),
        "csvs" => widgets::mono(plan.csv_names.join(", ")),
        "approval" => div()
            .text_color(if plan.approval == crate::olm::model::Approval::Manual {
                colors.yellow
            } else {
                colors.text_muted
            })
            .child(plan.approval.label())
            .into_any_element(),
        "phase" => widgets::pill(plan.phase.clone(), phase_tone(plan), colors),
        "age" => div()
            .font_family(fonts::MONO)
            .text_size(u(12.0))
            .text_color(colors.text_muted)
            .child(widgets::ago(plan.created))
            .into_any_element(),
        _ => div().into_any_element(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn plan(name: &str, phase: &str, approved: bool) -> Arc<InstallPlan> {
        Arc::new(
            InstallPlan::parse(&json!({"metadata": {"name": name, "namespace": "ops"},
                "spec": {"approved": approved}, "status": {"phase": phase}}))
            .unwrap(),
        )
    }

    #[test]
    fn pending_plans_come_first() {
        let plans = [
            plan("done", "Complete", true),
            plan("waiting", "RequiresApproval", false),
            plan("failed", "Failed", true),
        ];
        let rows = rows(&plans);
        let labels: Vec<String> = rows
            .iter()
            .map(|r| match r {
                PlanRow::Group(g) => g.clone(),
                PlanRow::Plan(p) => p.name.clone(),
            })
            .collect();
        assert_eq!(
            labels,
            [
                "Waiting for approval · 1",
                "waiting",
                "Installing and failed · 1",
                "failed",
                "Complete · 1",
                "done"
            ]
        );
    }
}
