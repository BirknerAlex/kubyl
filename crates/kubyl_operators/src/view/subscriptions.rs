//! Subscriptions: every Subscription with its channel, catalog, approval, CSVs and state.

use std::ops::Range;
use std::sync::Arc;

use gpui::{
    AnyElement, App, Context, FontWeight, IntoElement, Window, div, prelude::*, uniform_list,
};
use kubyl_core::actions::OpenView;
use kubyl_core::{ColumnDef, ColumnWidth, ResourceRef, Tone, ViewKind, ViewRequest};
use kubyl_ui::{ActiveColors, Button, Colors, IconName, fonts, h_flex, u, v_flex};

use super::{OperatorsView, SubTab};
use crate::olm::model::{Approval, Subscription};
use crate::widgets;

const ROW_HEIGHT: f32 = 32.0;

pub fn subscription_key(sub: &Subscription) -> String {
    format!("sub:{}/{}", sub.namespace, sub.name)
}

/// `AtLatestKnown`, `UpgradePending`… or `ResolutionFailed` when a condition says so.
fn state(sub: &Subscription) -> (String, Tone) {
    if sub.failure().is_some() {
        let reason = [
            "ResolutionFailed",
            "InstallPlanFailed",
            "BundleUnpackFailed",
        ]
        .into_iter()
        .find(|k| sub.condition(k).is_some())
        .unwrap_or("Failed");
        return (reason.to_string(), Tone::Bad);
    }
    match sub.state.as_deref() {
        Some("AtLatestKnown") => ("AtLatestKnown".into(), Tone::Good),
        Some(state @ ("UpgradePending" | "UpgradeAvailable")) => (state.into(), Tone::Warning),
        Some(state) => (state.into(), Tone::Info),
        None => ("Unknown".into(), Tone::Muted),
    }
}

impl OperatorsView {
    pub(crate) fn selected_subscription(&self, cx: &App) -> Option<Arc<Subscription>> {
        let key = self.selected.get(&SubTab::Subscriptions)?;
        self.snapshot(cx)?
            .subscriptions
            .iter()
            .find(|s| &subscription_key(s) == key)
            .cloned()
    }

    fn subscription_columns(&self) -> Vec<ColumnDef> {
        let details = self.details_open && self.selected_key().is_some();
        let mut columns = vec![
            ColumnDef::new(
                "name",
                "Name",
                ColumnWidth::Flex {
                    weight: 1.0,
                    min: 140.0,
                },
            ),
            ColumnDef::new("namespace", "Namespace", ColumnWidth::Fixed(110.0)),
            ColumnDef::new("channel", "Channel", ColumnWidth::Fixed(96.0)),
        ];
        if !details {
            columns.push(ColumnDef::new(
                "catalog",
                "Catalog",
                ColumnWidth::Flex {
                    weight: 1.0,
                    min: 120.0,
                },
            ));
        }
        columns.extend([
            ColumnDef::new("approval", "Approval", ColumnWidth::Fixed(86.0)),
            ColumnDef::new(
                "installed",
                "Installed CSV",
                ColumnWidth::Flex {
                    weight: 1.3,
                    min: 150.0,
                },
            ),
            ColumnDef::new("state", "State", ColumnWidth::Fixed(140.0)),
        ]);
        columns
    }

    pub(crate) fn render_subscriptions(
        &mut self,
        _: &mut Window,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let colors = cx.colors().clone();
        let Some(snapshot) = self.snapshot(cx) else {
            return widgets::empty("Loading…", &colors);
        };
        let query = self.query(cx);
        self.subscription_rows = snapshot
            .subscriptions
            .iter()
            .filter(|s| {
                query.is_empty()
                    || Self::matches(
                        &query,
                        &format!(
                            "{} {} {} {} {}",
                            s.name,
                            s.namespace,
                            s.package,
                            s.source,
                            s.installed_csv.as_deref().unwrap_or_default()
                        ),
                    )
            })
            .cloned()
            .collect();
        self.keys = self
            .subscription_rows
            .iter()
            .map(|s| subscription_key(s))
            .collect();
        if self.selected_key().is_none()
            && let Some(first) = self.keys.first().cloned()
        {
            self.selected.insert(SubTab::Subscriptions, first);
        }
        let body = if self.subscription_rows.is_empty() {
            widgets::empty(
                if snapshot.subscriptions.is_empty() {
                    "No subscriptions."
                } else {
                    "No subscriptions match the filter."
                },
                &colors,
            )
        } else {
            let columns = self.subscription_columns();
            v_flex()
                .flex_1()
                .min_h_0()
                .child(widgets::header(&columns, &colors))
                .child(
                    uniform_list(
                        "subscription-rows",
                        self.subscription_rows.len(),
                        cx.processor(|this, range: Range<usize>, _, cx| {
                            this.render_subscription_rows(range, cx)
                        }),
                    )
                    .flex_1()
                    .track_scroll(&self.scroll),
                )
                .into_any_element()
        };
        let details = self
            .details_open
            .then(|| self.selected_subscription(cx))
            .flatten()
            .map(|sub| self.render_subscription_details(sub, cx));
        self.focus_area()
            .items_start()
            .child(v_flex().flex_1().min_w_0().h_full().child(body))
            .children(details)
            .into_any_element()
    }

    fn render_subscription_rows(
        &mut self,
        range: Range<usize>,
        cx: &mut Context<Self>,
    ) -> Vec<AnyElement> {
        let colors = cx.colors().clone();
        let columns = self.subscription_columns();
        let selected = self.selected_key().cloned();
        range
            .filter_map(|index| {
                let sub = self.subscription_rows.get(index)?.clone();
                let key = subscription_key(&sub);
                Some(
                    widgets::row(
                        ("sub-row", index),
                        selected.as_ref() == Some(&key),
                        ROW_HEIGHT,
                        &colors,
                    )
                    .children(columns.iter().map(|def| {
                        widgets::column_cell(def).child(subscription_cell(
                            &sub,
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
                    .into_any_element(),
                )
            })
            .collect()
    }

    fn render_subscription_details(
        &mut self,
        sub: Arc<Subscription>,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let colors = cx.colors().clone();
        let read_only = self.read_only(cx);
        let (state_label, tone) = state(&sub);
        let cluster = self.cluster.clone();
        let mut rows = vec![
            ("Package", widgets::text(sub.package.clone())),
            (
                "Channel",
                widgets::text(sub.channel.clone().unwrap_or_else(|| "default".into())),
            ),
            (
                "Catalog",
                widgets::text(format!("{} · {}", sub.source, sub.source_namespace)),
            ),
        ];
        if let Some(start) = &sub.starting_csv {
            rows.push(("Starting CSV", widgets::mono(start.clone())));
        }
        rows.push((
            "Installed CSV",
            widgets::mono(sub.installed_csv.clone().unwrap_or_else(|| "—".into())),
        ));
        if sub.current_csv != sub.installed_csv
            && let Some(current) = &sub.current_csv
        {
            rows.push(("Latest CSV", widgets::mono(current.clone())));
        }
        rows.push((
            "Approval",
            div()
                .text_color(if sub.approval == Approval::Manual {
                    colors.yellow
                } else {
                    colors.text
                })
                .child(sub.approval.label())
                .into_any_element(),
        ));
        if let Some(plan) = &sub.install_plan {
            let key = format!("plan:{}/{plan}", sub.namespace);
            let weak = cx.entity().downgrade();
            rows.push((
                "Install plan",
                widgets::link("sub-plan", plan.clone(), &colors, move |_, window, cx| {
                    let key = key.clone();
                    weak.update(cx, |this, cx| {
                        this.set_tab(SubTab::Plans, window, cx);
                        this.selected.insert(SubTab::Plans, key);
                        cx.notify();
                    })
                    .ok();
                })
                .into_any_element(),
            ));
        }
        let mut conditions = widgets::section("Conditions", &colors);
        if sub.conditions.is_empty() {
            conditions = conditions.child(
                div()
                    .text_size(u(12.0))
                    .text_color(colors.text_dim)
                    .child("None reported."),
            );
        }
        for condition in &sub.conditions {
            // CatalogSourcesUnhealthy=False is good news; the others are good when True.
            let good = match condition.kind.as_str() {
                "CatalogSourcesUnhealthy"
                | "ResolutionFailed"
                | "InstallPlanFailed"
                | "InstallPlanMissing"
                | "BundleUnpackFailed" => !condition.is_true(),
                _ => condition.is_true(),
            };
            let pending =
                condition.kind == "InstallPlanPending" || condition.kind == "BundleUnpacking";
            let (icon, color) = if pending && condition.is_true() {
                (IconName::Clock, colors.yellow)
            } else if good {
                (IconName::CircleCheck, colors.green)
            } else {
                (IconName::CircleX, colors.red)
            };
            let text = format!(
                "{} · {}{}",
                condition.kind,
                condition.status,
                condition
                    .message
                    .as_ref()
                    .or(condition.reason.as_ref())
                    .map(|m| format!(" · {m}"))
                    .unwrap_or_default()
            );
            conditions = conditions.child(widgets::note(icon, color, text, &colors));
        }
        let yaml_ref = ResourceRef::object(
            cluster.clone(),
            crate::olm::model::subscriptions(),
            Some(sub.namespace.clone()),
            sub.name.clone(),
        );
        let hub_cluster = cluster.clone();
        let package = sub.package.clone();
        let operator = self.snapshot(cx).and_then(|s| {
            s.operators
                .iter()
                .find(|o| {
                    o.subscription
                        .as_ref()
                        .is_some_and(|x| x.key() == sub.key())
                })
                .cloned()
        });
        v_flex()
            .flex_none()
            .w(u(350.0))
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
                            .child(sub.name.clone()),
                    )
                    .child(
                        div()
                            .text_size(u(11.5))
                            .text_color(colors.text_dim)
                            .child(sub.namespace.clone()),
                    )
                    .child(
                        kubyl_ui::IconButton::new("sub-details-close", IconName::X)
                            .icon_size(13.0)
                            .on_click(cx.listener(|this, _, _, cx| {
                                this.details_open = false;
                                cx.notify();
                            })),
                    ),
            )
            .child(
                v_flex()
                    .id("sub-details")
                    .flex_1()
                    .min_h_0()
                    .overflow_y_scroll()
                    .child(
                        widgets::plain_section(&colors)
                            .child(widgets::pill(state_label, tone, &colors))
                            .when_some(sub.failure(), |this, failure| {
                                this.child(
                                    div()
                                        .text_size(u(12.0))
                                        .text_color(colors.text_muted)
                                        .child(failure),
                                )
                            })
                            .child(widgets::kv(rows, &colors)),
                    )
                    .child(conditions)
                    .child(
                        widgets::plain_section(&colors).child(
                            h_flex()
                                .gap(u(6.0))
                                .child(
                                    Button::new("sub-yaml")
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
                                )
                                .child(
                                    Button::new("sub-hub")
                                        .ghost()
                                        .icon(IconName::Store)
                                        .label("OperatorHub")
                                        .on_click(move |_, window, cx| {
                                            crate::hub::open_package(
                                                &hub_cluster,
                                                &package,
                                                window,
                                                cx,
                                            )
                                        }),
                                )
                                .child(div().flex_1())
                                .when_some(operator.filter(|_| !read_only), |this, operator| {
                                    let cluster = cluster.clone();
                                    this.child(
                                        Button::new("sub-uninstall")
                                            .danger()
                                            .icon(IconName::Trash)
                                            .label("Uninstall…")
                                            .on_click(move |_, window, cx| {
                                                crate::dialogs::open_uninstall(
                                                    cluster.clone(),
                                                    operator.clone(),
                                                    window,
                                                    cx,
                                                )
                                            }),
                                    )
                                }),
                        ),
                    ),
            )
            .into_any_element()
    }
}

fn subscription_cell(sub: &Subscription, column: &str, colors: &Colors) -> AnyElement {
    let dim = |text: String| {
        div()
            .truncate()
            .font_family(fonts::MONO)
            .text_size(u(12.0))
            .text_color(colors.text_muted)
            .child(text)
            .into_any_element()
    };
    match column {
        "name" => widgets::mono(sub.name.clone()),
        "namespace" => dim(sub.namespace.clone()),
        "channel" => dim(sub.channel.clone().unwrap_or_else(|| "default".into())),
        "catalog" => dim(sub.source.clone()),
        "approval" => div()
            .text_color(if sub.approval == Approval::Manual {
                colors.yellow
            } else {
                colors.text_muted
            })
            .child(sub.approval.label())
            .into_any_element(),
        "installed" => widgets::mono(sub.installed_csv.clone().unwrap_or_else(|| "—".into())),
        "state" => {
            let (label, tone) = state(sub);
            widgets::pill(label, tone, colors)
        }
        _ => div().into_any_element(),
    }
}
