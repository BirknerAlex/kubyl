//! Installed operators: the list and an operator's details (the pending upgrade, provided APIs
//! with instance counts and Create, the subscription).

use std::ops::Range;
use std::sync::Arc;

use gpui::{
    AnyElement, App, Context, FontWeight, IntoElement, SharedString, Subscription, Window, div,
    prelude::*, uniform_list,
};
use kubyl_core::actions::OpenView;
use kubyl_core::{ColumnDef, ColumnWidth, Gvr, ResourceRef, ViewKind, ViewRegistry, ViewRequest};
use kubyl_kube::ConnectionManager;
use kubyl_resources::{ResourceStores, StoreHandle, StoreKey};
use kubyl_ui::{ActiveColors, Button, Chip, Colors, Icon, IconName, fonts, h_flex, u, v_flex};

use super::OperatorsView;
use crate::olm::join::{Operator, OperatorStatus};
use crate::olm::model::OwnedCrd;
use crate::olm::review::Level;
use crate::service::{Olm, ReviewState};
use crate::widgets;

const ROW_HEIGHT: f32 = 46.0;

/// The instance counts of the selected operator's APIs (metadata watches).
pub(crate) struct Instances {
    key: String,
    stores: Vec<(OwnedCrd, Option<(Gvr, StoreHandle)>)>,
    _observers: Vec<Subscription>,
}

impl OperatorsView {
    pub(crate) fn selected_operator(&self, cx: &App) -> Option<Operator> {
        let key = self.selected.get(&super::SubTab::Installed)?;
        self.snapshot(cx)?.operator(key).cloned()
    }

    fn installed_columns(&self) -> Vec<ColumnDef> {
        let flex = |weight: f32, min: f32| ColumnWidth::Flex { weight, min };
        let details = self.details_open && self.selected_key().is_some();
        let mut columns = vec![
            ColumnDef::new("name", "Name", flex(1.3, 190.0)),
            ColumnDef::new("version", "Version", ColumnWidth::Fixed(84.0)),
            ColumnDef::new("status", "Status", ColumnWidth::Fixed(150.0)),
            ColumnDef::new("channel", "Channel", ColumnWidth::Fixed(96.0)),
            ColumnDef::new("approval", "Approval", ColumnWidth::Fixed(86.0)),
            ColumnDef::new("namespace", "Namespace", ColumnWidth::Fixed(110.0)),
        ];
        columns.push(ColumnDef::new(
            "apis",
            "Provided APIs",
            flex(1.1, if details { 60.0 } else { 140.0 }),
        ));
        columns
    }

    pub(crate) fn render_installed(
        &mut self,
        _: &mut Window,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let colors = cx.colors().clone();
        let Some(snapshot) = self.snapshot(cx) else {
            return widgets::empty("Loading…", &colors);
        };
        if !snapshot.v0 {
            return widgets::empty("This cluster runs OLM v1 only: see Extensions.", &colors);
        }
        let query = self.query(cx);
        self.installed_rows = snapshot
            .operators
            .iter()
            .filter(|o| {
                query.is_empty()
                    || Self::matches(
                        &query,
                        &format!(
                            "{} {} {} {} {}",
                            o.display_name(),
                            o.package().unwrap_or_default(),
                            o.namespace(),
                            o.status.label(),
                            o.csv
                                .as_ref()
                                .map(|c| c
                                    .owned
                                    .iter()
                                    .map(|k| k.kind.as_str())
                                    .collect::<Vec<_>>()
                                    .join(" "))
                                .unwrap_or_default()
                        ),
                    )
            })
            .cloned()
            .collect();
        self.keys = self.installed_rows.iter().map(|o| o.key.clone()).collect();
        if self.selected_key().is_none()
            && let Some(first) = self.keys.first().cloned()
        {
            self.selected.insert(super::SubTab::Installed, first);
        }
        let body = if self.installed_rows.is_empty() {
            widgets::empty(
                if snapshot.operators.is_empty() {
                    "No operators installed. Browse OperatorHub to install one."
                } else {
                    "No operators match the filter."
                },
                &colors,
            )
        } else {
            let columns = self.installed_columns();
            v_flex()
                .flex_1()
                .min_h_0()
                .child(widgets::header(&columns, &colors))
                .child(
                    uniform_list(
                        "operators-rows",
                        self.installed_rows.len(),
                        cx.processor(|this, range: Range<usize>, _, cx| {
                            this.render_installed_rows(range, cx)
                        }),
                    )
                    .flex_1()
                    .track_scroll(&self.scroll),
                )
                .into_any_element()
        };
        let details = (self.details_open)
            .then(|| self.selected_operator(cx))
            .flatten()
            .map(|op| self.render_operator_details(op, cx));
        self.focus_area()
            .items_start()
            .child(v_flex().flex_1().min_w_0().h_full().child(body))
            .children(details)
            .into_any_element()
    }

    fn render_installed_rows(
        &mut self,
        range: Range<usize>,
        cx: &mut Context<Self>,
    ) -> Vec<AnyElement> {
        let colors = cx.colors().clone();
        let columns = self.installed_columns();
        let selected = self.selected_key().cloned();
        range
            .filter_map(|index| {
                let op = self.installed_rows.get(index)?.clone();
                let key = op.key.clone();
                let is_selected = selected.as_ref() == Some(&key);
                Some(
                    widgets::row(("operator-row", index), is_selected, ROW_HEIGHT, &colors)
                        .children(columns.iter().map(|def| {
                            widgets::column_cell(def).child(self.installed_cell(
                                &op,
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

    fn installed_cell(&self, op: &Operator, column: &str, colors: &Colors) -> AnyElement {
        match column {
            "name" => h_flex()
                .gap(u(10.0))
                .min_w_0()
                .child(widgets::tile(
                    op.initials(),
                    &op.display_name(),
                    op.csv.as_ref().and_then(|c| widgets::csv_icon(&c.icon)),
                    26.0,
                    colors,
                ))
                .child(
                    v_flex()
                        .min_w_0()
                        .child(
                            div()
                                .truncate()
                                .font_weight(FontWeight::MEDIUM)
                                .child(op.display_name()),
                        )
                        .child(
                            div()
                                .truncate()
                                .font_family(fonts::MONO)
                                .text_size(u(11.0))
                                .text_color(colors.text_dim)
                                .child(
                                    op.package()
                                        .map(str::to_string)
                                        .or_else(|| op.csv.as_ref().map(|c| c.name.clone()))
                                        .unwrap_or_default(),
                                ),
                        ),
                )
                .into_any_element(),
            "version" => widgets::mono(op.version().unwrap_or("—").to_string()),
            "status" => {
                let label = match (&op.status, &op.upgrade_to) {
                    (OperatorStatus::UpgradeAvailable, Some(to)) => format!("Upgrade to {to}"),
                    _ => op.status.label().to_string(),
                };
                widgets::pill(label, op.status.tone(), colors)
            }
            "channel" => div()
                .truncate()
                .font_family(fonts::MONO)
                .text_size(u(12.0))
                .text_color(colors.text_muted)
                .child(op.channel().unwrap_or("—").to_string())
                .into_any_element(),
            "approval" => match op.approval() {
                Some(approval) => div()
                    .text_color(if approval == crate::olm::model::Approval::Manual {
                        colors.yellow
                    } else {
                        colors.text_muted
                    })
                    .child(approval.label())
                    .into_any_element(),
                None => div()
                    .text_color(colors.text_dim)
                    .child("—")
                    .into_any_element(),
            },
            "namespace" => div()
                .truncate()
                .font_family(fonts::MONO)
                .text_size(u(11.5))
                .text_color(colors.text_muted)
                .child(op.namespace().to_string())
                .into_any_element(),
            "apis" => {
                let owned = op.csv.as_ref().map(|c| c.owned.clone()).unwrap_or_default();
                let shown = owned.len().min(3);
                h_flex()
                    .gap(u(4.0))
                    .overflow_hidden()
                    .children(
                        owned
                            .iter()
                            .take(shown)
                            .map(|crd| Chip::new(crd.kind.clone())),
                    )
                    .when(owned.len() > shown, |this| {
                        this.child(Chip::new(format!("+{}", owned.len() - shown)))
                    })
                    .into_any_element()
            }
            _ => div().into_any_element(),
        }
    }

    /// Keeps metadata watches on the selected operator's kinds, for the instance counts.
    fn ensure_instances(&mut self, op: &Operator, cx: &mut Context<Self>) {
        if self.instances.as_ref().is_some_and(|i| i.key == op.key) {
            return;
        }
        let discovery =
            ConnectionManager::try_global(cx).and_then(|m| m.read(cx).discovery(&self.cluster));
        let mut observers = Vec::new();
        let stores = op
            .csv
            .as_ref()
            .map(|c| c.owned.clone())
            .unwrap_or_default()
            .into_iter()
            .map(|crd| {
                let gvr = discovery.as_ref().and_then(|d| {
                    kubyl_explorer::catalog::find(d, crd.group(), crd.plural())
                        .map(|i| i.gvr.clone())
                });
                let store = gvr.map(|gvr| {
                    let handle = ResourceStores::acquire(
                        cx,
                        StoreKey::new(self.cluster.clone(), gvr.clone(), None).metadata(),
                    );
                    observers.push(cx.observe(handle.entity(), |_, _, cx| cx.notify()));
                    (gvr, handle)
                });
                (crd, store)
            })
            .collect();
        self.instances = Some(Instances {
            key: op.key.clone(),
            stores,
            _observers: observers,
        });
    }

    fn render_operator_details(&mut self, op: Operator, cx: &mut Context<Self>) -> AnyElement {
        self.ensure_instances(&op, cx);
        let colors = cx.colors().clone();
        let read_only = self.read_only(cx);
        let cluster = self.cluster.clone();
        let subtitle = [
            op.version().map(str::to_string),
            Some(op.namespace().to_string()),
            op.approval().map(|a| format!("{} approval", a.label())),
        ]
        .into_iter()
        .flatten()
        .collect::<Vec<_>>()
        .join(" · ");
        let header = h_flex()
            .flex_none()
            .px(u(12.0))
            .py(u(8.0))
            .gap(u(8.0))
            .border_b_1()
            .border_color(colors.border_variant)
            .child(widgets::tile(
                op.initials(),
                &op.display_name(),
                op.csv.as_ref().and_then(|c| widgets::csv_icon(&c.icon)),
                26.0,
                &colors,
            ))
            .child(
                v_flex()
                    .flex_1()
                    .min_w_0()
                    .child(
                        div()
                            .truncate()
                            .font_weight(FontWeight::MEDIUM)
                            .child(op.display_name()),
                    )
                    .child(
                        div()
                            .truncate()
                            .text_size(u(11.0))
                            .text_color(colors.text_dim)
                            .child(subtitle),
                    ),
            )
            .child(
                kubyl_ui::IconButton::new("operator-details-close", IconName::X)
                    .icon_size(13.0)
                    .on_click(cx.listener(|this, _, _, cx| {
                        this.details_open = false;
                        cx.notify();
                    })),
            );
        let mut sections: Vec<AnyElement> = Vec::new();
        // Status, when there's something to say (a pending plan has its own card).
        let pending = op.approvable().is_some();
        if !pending && (op.status != OperatorStatus::Succeeded || op.detail.is_some()) {
            let status = widgets::plain_section(&colors)
                .child(widgets::pill(op.status.label(), op.status.tone(), &colors))
                .when_some(op.detail.clone(), |this, detail| {
                    this.child(
                        div()
                            .text_size(u(12.0))
                            .text_color(colors.text_muted)
                            .child(detail),
                    )
                });
            sections.push(status.into_any_element());
        }
        if let Some(plan) = op.plan.clone() {
            sections.push(self.render_upgrade_card(&op, plan, read_only, cx));
        }
        // Provided APIs.
        if let (Some(csv), Some(instances)) = (op.csv.clone(), self.instances.as_ref()) {
            let mut apis = widgets::section("Provided APIs", &colors);
            if instances.stores.is_empty() {
                apis = apis.child(
                    div()
                        .text_size(u(12.0))
                        .text_color(colors.text_dim)
                        .child("This operator doesn't own any CRDs."),
                );
            }
            for (i, (crd, store)) in instances.stores.iter().enumerate() {
                let count = match store {
                    Some((_, handle)) => {
                        let store = handle.read(cx);
                        match store.status() {
                            kubyl_resources::StoreStatus::Forbidden => "no access".to_string(),
                            status if !status.is_settled() => "…".to_string(),
                            _ => match store.len() {
                                0 => "none".to_string(),
                                1 => "1 instance".to_string(),
                                n => format!("{n} instances"),
                            },
                        }
                    }
                    None => "not served".to_string(),
                };
                let list_target = store
                    .as_ref()
                    .map(|(gvr, _)| ResourceRef::list(cluster.clone(), gvr.clone(), None));
                let create_cluster = cluster.clone();
                let create_csv = csv.clone();
                let create_crd = crd.clone();
                apis = apis.child(
                    h_flex()
                        .gap(u(8.0))
                        .px(u(10.0))
                        .py(u(6.0))
                        .rounded(u(6.0))
                        .border_1()
                        .border_color(colors.border_variant)
                        .bg(colors.subheader_background)
                        .child(
                            v_flex()
                                .flex_1()
                                .min_w_0()
                                .child(
                                    div()
                                        .truncate()
                                        .font_family(fonts::MONO)
                                        .text_size(u(12.0))
                                        .child(crd.kind.clone()),
                                )
                                .child(
                                    div()
                                        .truncate()
                                        .font_family(fonts::MONO)
                                        .text_size(u(10.5))
                                        .text_color(colors.text_dim)
                                        .child(format!("{}/{}", crd.group(), crd.version)),
                                ),
                        )
                        .child(match list_target {
                            Some(target) => widgets::link(
                                ("api-instances", i),
                                count,
                                &colors,
                                move |_, window, cx| {
                                    let kind = ViewRegistry::list_view(cx, &target.gvr);
                                    window.dispatch_action(
                                        Box::new(OpenView(ViewRequest::for_resource(
                                            kind,
                                            target.clone(),
                                        ))),
                                        cx,
                                    );
                                },
                            )
                            .flex_none()
                            .text_size(u(11.5))
                            .into_any_element(),
                            None => div()
                                .flex_none()
                                .text_size(u(11.5))
                                .text_color(colors.text_dim)
                                .child(count)
                                .into_any_element(),
                        })
                        .when(!read_only, |this| {
                            this.child(
                                Button::new(("api-create", i))
                                    .ghost()
                                    .icon(IconName::Plus)
                                    .label("Create")
                                    .on_click(move |_, window, cx| {
                                        crate::dialogs::create_instance(
                                            &create_cluster,
                                            &create_csv,
                                            &create_crd,
                                            window,
                                            cx,
                                        )
                                    }),
                            )
                        }),
                );
            }
            sections.push(apis.into_any_element());
        }
        // Subscription.
        if let Some(sub) = &op.subscription {
            let catalog = format!("{} · {}", sub.source, sub.source_namespace);
            let group = op.csv.as_ref().and_then(|c| {
                let group = c.operator_group.clone()?;
                let mode = match c.target_namespaces.as_deref() {
                    None | Some("") => "all namespaces".to_string(),
                    Some(targets) => targets.replace(',', ", "),
                };
                Some(format!("{}/{group} · {mode}", c.namespace))
            });
            let mut rows = vec![
                ("Catalog", widgets::text(catalog)),
                (
                    "Channel",
                    widgets::text(sub.channel.clone().unwrap_or_else(|| "default".into())),
                ),
                (
                    "Installed CSV",
                    widgets::mono(sub.installed_csv.clone().unwrap_or_else(|| "—".into())),
                ),
            ];
            if sub.current_csv != sub.installed_csv
                && let Some(current) = &sub.current_csv
            {
                rows.push(("Latest CSV", widgets::mono(current.clone())));
            }
            rows.push((
                "Approval",
                div()
                    .text_color(if sub.approval == crate::olm::model::Approval::Manual {
                        colors.yellow
                    } else {
                        colors.text
                    })
                    .child(sub.approval.label())
                    .into_any_element(),
            ));
            if let Some(group) = group {
                rows.push(("Watches", widgets::mono(group)));
            }
            if let Some(healthy) = sub.catalog_healthy.filter(|h| !h) {
                let _ = healthy;
                rows.push((
                    "Catalog health",
                    div()
                        .text_color(colors.red)
                        .child("unhealthy")
                        .into_any_element(),
                ));
            }
            sections.push(
                widgets::section("Subscription", &colors)
                    .child(widgets::kv(rows, &colors))
                    .into_any_element(),
            );
        }
        // Actions.
        let csv_ref = op.csv.as_ref().map(|c| {
            ResourceRef::object(
                cluster.clone(),
                crate::olm::model::csvs(),
                Some(c.namespace.clone()),
                c.name.clone(),
            )
        });
        let logs_ref = op.csv.as_ref().and_then(|c| {
            Some(ResourceRef::object(
                cluster.clone(),
                Gvr::new("apps", "v1", "deployments"),
                Some(c.namespace.clone()),
                c.deployments.first()?.clone(),
            ))
        });
        let uninstall_op = op.clone();
        let uninstall_cluster = cluster.clone();
        let busy =
            Olm::global(cx).is_some_and(|o| o.read(cx).is_busy(&format!("uninstall:{}", op.key)));
        sections.push(
            widgets::plain_section(&colors)
                .child(
                    h_flex()
                        .gap(u(6.0))
                        .when_some(csv_ref, |this, target| {
                            this.child(
                                Button::new("csv-yaml")
                                    .ghost()
                                    .icon(IconName::Code)
                                    .label("CSV YAML")
                                    .on_click(move |_, window, cx| {
                                        window.dispatch_action(
                                            Box::new(OpenView(ViewRequest::for_resource(
                                                ViewKind::Yaml,
                                                target.clone(),
                                            ))),
                                            cx,
                                        )
                                    }),
                            )
                        })
                        .when_some(logs_ref, |this, target| {
                            this.child(
                                Button::new("operator-logs")
                                    .ghost()
                                    .icon(IconName::List)
                                    .label("Logs")
                                    .on_click(move |_, window, cx| {
                                        window.dispatch_action(
                                            Box::new(OpenView(ViewRequest::for_resource(
                                                ViewKind::Logs,
                                                target.clone(),
                                            ))),
                                            cx,
                                        )
                                    }),
                            )
                        })
                        .child(div().flex_1())
                        .when(!read_only, |this| {
                            this.child(
                                Button::new("operator-uninstall")
                                    .danger()
                                    .icon(IconName::Trash)
                                    .label(if busy {
                                        "Uninstalling…"
                                    } else {
                                        "Uninstall…"
                                    })
                                    .disabled(busy)
                                    .on_click(move |_, window, cx| {
                                        crate::dialogs::open_uninstall(
                                            uninstall_cluster.clone(),
                                            uninstall_op.clone(),
                                            window,
                                            cx,
                                        )
                                    }),
                            )
                        }),
                )
                .into_any_element(),
        );
        v_flex()
            .flex_none()
            .w(u(360.0))
            .h_full()
            .bg(colors.panel)
            .border_l_1()
            .border_color(colors.border)
            .child(header)
            .child(
                v_flex()
                    .id("operator-details")
                    .flex_1()
                    .min_h_0()
                    .overflow_y_scroll()
                    .children(sections),
            )
            .into_any_element()
    }

    fn render_upgrade_card(
        &mut self,
        op: &Operator,
        plan: Arc<crate::olm::model::InstallPlan>,
        read_only: bool,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let colors = cx.colors().clone();
        let waiting = plan.needs_approval();
        let title: SharedString = match (waiting, op.csv.is_some()) {
            (true, true) => "Upgrade waiting for approval".into(),
            (true, false) => "Install waiting for approval".into(),
            (false, _) if plan.failed() => "Install plan failed".into(),
            (false, _) => format!("Install plan {}", plan.phase.to_lowercase()).into(),
        };
        let tint = if plan.failed() {
            colors.red
        } else {
            colors.yellow
        };
        let from = op.version().unwrap_or("—").to_string();
        let to = op
            .upgrade_to
            .clone()
            .unwrap_or_else(|| plan.csv_names.join(", "));
        let review = if waiting {
            Olm::global(cx).map(|olm| {
                olm.update(cx, |olm, cx| {
                    olm.review(&self.cluster, &plan, op.csv.clone(), cx)
                })
            })
        } else {
            None
        };
        let mut lines: Vec<AnyElement> = Vec::new();
        match review {
            Some(ReviewState::Loading) => lines.push(
                div()
                    .text_size(u(12.0))
                    .text_color(colors.text_dim)
                    .child("Reading the plan's changes…")
                    .into_any_element(),
            ),
            Some(ReviewState::Ready(review)) => {
                let changed: Vec<&str> = review
                    .crds
                    .iter()
                    .filter(|c| c.status != crate::olm::review::CrdStatus::Unchanged)
                    .map(|c| c.name.as_str())
                    .collect();
                if !review.crds.is_empty() {
                    lines.push(widgets::note(
                        IconName::File,
                        colors.accent,
                        if changed.is_empty() {
                            format!("{} CRDs, none change", review.crds.len())
                        } else {
                            format!(
                                "{} of {} CRDs change: {}",
                                changed.len(),
                                review.crds.len(),
                                changed
                                    .iter()
                                    .take(2)
                                    .copied()
                                    .collect::<Vec<_>>()
                                    .join(", ")
                            )
                        },
                        &colors,
                    ));
                }
                let gained = review.rbac.iter().filter(|r| r.added).count();
                let lost = review.rbac.len() - gained;
                lines.push(widgets::note(
                    IconName::Shield,
                    if gained > 0 {
                        colors.yellow
                    } else {
                        colors.text_dim
                    },
                    match (gained, lost) {
                        (0, 0) => "RBAC: no permission changes".to_string(),
                        _ => {
                            let first = review.rbac.first().map(|r| {
                                format!(
                                    " (first: {} {} {})",
                                    if r.added { "+" } else { "−" },
                                    r.verbs.join(","),
                                    r.target
                                )
                            });
                            format!(
                                "RBAC: {gained} rules gained, {lost} lost{}",
                                first.unwrap_or_default()
                            )
                        }
                    },
                    &colors,
                ));
                for note in review.notes.iter().filter(|n| n.level != Level::Ok).take(2) {
                    lines.push(widgets::note(
                        if note.level == Level::Blocker {
                            IconName::CircleX
                        } else {
                            IconName::TriangleAlert
                        },
                        if note.level == Level::Blocker {
                            colors.red
                        } else {
                            colors.yellow
                        },
                        note.text.clone(),
                        &colors,
                    ));
                }
                if review.notes.iter().all(|n| n.level == Level::Ok)
                    && let Some(ok) = review.notes.first()
                {
                    lines.push(widgets::note(
                        IconName::CircleCheck,
                        colors.green,
                        ok.text.clone(),
                        &colors,
                    ));
                }
                for problem in review.problems.iter().take(1) {
                    lines.push(widgets::note(
                        IconName::TriangleAlert,
                        colors.yellow,
                        problem.clone(),
                        &colors,
                    ));
                }
            }
            None => {
                if let Some(failure) = plan.failure() {
                    lines.push(
                        div()
                            .text_size(u(12.0))
                            .text_color(colors.text_muted)
                            .child(failure)
                            .into_any_element(),
                    );
                }
            }
        }
        let cluster = self.cluster.clone();
        let review_plan = plan.clone();
        let approve_plan = plan.clone();
        let approve_cluster = cluster.clone();
        let yaml_ref = ResourceRef::object(
            cluster.clone(),
            crate::olm::model::install_plans(),
            Some(plan.namespace.clone()),
            plan.name.clone(),
        );
        let busy =
            Olm::global(cx).is_some_and(|o| o.read(cx).is_busy(&format!("plan:{}", plan.key())));
        v_flex()
            .px(u(14.0))
            .py(u(12.0))
            .gap(u(6.0))
            .bg(tint.opacity(0.1))
            .border_b_1()
            .border_color(colors.border_variant)
            .child(
                h_flex()
                    .gap(u(8.0))
                    .child(Icon::new(IconName::ArrowUp).size(14.0).color(tint))
                    .child(
                        div()
                            .font_weight(FontWeight::SEMIBOLD)
                            .text_color(tint)
                            .child(title),
                    ),
            )
            .child(
                h_flex()
                    .gap(u(6.0))
                    .font_family(fonts::MONO)
                    .text_size(u(12.0))
                    .child(from)
                    .child(div().text_color(colors.text_dim).child("→"))
                    .child(div().text_color(colors.green).child(to))
                    .child(
                        div()
                            .text_color(colors.text_dim)
                            .child(format!("· {}", plan.name)),
                    ),
            )
            .children(lines)
            .child(
                h_flex()
                    .pt(u(4.0))
                    .gap(u(6.0))
                    .when(waiting && !read_only, |this| {
                        this.child(
                            Button::new("upgrade-approve")
                                .primary()
                                .icon(IconName::Check)
                                .label(if busy { "Approving…" } else { "Approve…" })
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
                    .child(
                        Button::new("upgrade-review")
                            .icon(IconName::Diff)
                            .label("Review changes")
                            .on_click(move |_, window, cx| {
                                crate::dialogs::open_review(
                                    cluster.clone(),
                                    review_plan.clone(),
                                    window,
                                    cx,
                                )
                            }),
                    )
                    .child(
                        Button::new("upgrade-yaml")
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
            )
            .into_any_element()
    }
}
