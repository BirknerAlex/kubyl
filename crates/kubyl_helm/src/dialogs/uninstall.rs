//! Uninstall a release (board 20): the release's objects that get deleted, what stays (objects
//! with `helm.sh/resource-policy: keep`, PVCs of StatefulSet templates, the chart's CRDs), the
//! hooks that run; options; a typed name on production clusters; then `helm uninstall`.

use gpui::{
    AnyElement, App, AppContext as _, Context, Entity, FocusHandle, Focusable, IntoElement, Render,
    Subscription, Task, Window, div, prelude::*,
};
use gpui_component::input::{InputEvent, InputState};
use kubyl_core::{ClusterId, spawn_kube};
use kubyl_helm_core::cli::HelmInfo;
use kubyl_helm_core::cmd::{self, UninstallSpec};
use kubyl_helm_core::preview::{self, UninstallPlan};
use kubyl_ui::{ActiveColors, Button, Icon, IconName, ProdBadge, fonts, h_flex, u, v_flex};

use super::{checkbox, client, error_line, footer, guard, header, open, production, typed_input};
use crate::cli::HelmCli;
use crate::ops::{HelmOps, OpKind, Start};
use crate::service::ReleaseRow;
use crate::widgets;

/// Opens the uninstall dialog for a release.
pub fn open_uninstall(cluster: ClusterId, row: ReleaseRow, window: &mut Window, cx: &mut App) {
    if !guard(&cluster, cx) {
        return;
    }
    let Some(helm) = HelmCli::info(cx) else {
        super::open_missing(window, cx);
        return;
    };
    let view = cx.new(|cx| UninstallDialog::new(cluster, row, helm, window, cx));
    let prod = production(&view.read(cx).cluster, cx);
    let focus = if prod {
        view.read(cx).typed.read(cx).focus_handle(cx)
    } else {
        view.read(cx).focus.clone()
    };
    open(view, 780.0, Some(focus), window, cx);
}

enum Plan {
    Loading(#[allow(dead_code)] Task<()>),
    Ready(UninstallPlan),
    Failed(String),
}

pub struct UninstallDialog {
    cluster: ClusterId,
    row: ReleaseRow,
    helm: HelmInfo,
    plan: Plan,
    keep_history: bool,
    no_hooks: bool,
    wait: bool,
    typed: Entity<InputState>,
    running: Option<u64>,
    error: Option<String>,
    focus: FocusHandle,
    _subscriptions: Vec<Subscription>,
}

impl UninstallDialog {
    fn new(
        cluster: ClusterId,
        row: ReleaseRow,
        helm: HelmInfo,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Self {
        let typed = typed_input(row.name.clone(), window, cx);
        let mut subscriptions = vec![cx.subscribe_in(
            &typed,
            window,
            |this, _, event: &InputEvent, _, cx| match event {
                InputEvent::Change => {
                    this.error = None;
                    cx.notify();
                }
                InputEvent::PressEnter { .. } => this.run(cx),
                _ => {}
            },
        )];
        if let Some(ops) = HelmOps::global(cx) {
            subscriptions.push(cx.observe(&ops, |_, _, cx| cx.notify()));
        }
        let plan = match client(&cluster, cx) {
            Some(client) => {
                let (driver, namespace, object) = (
                    row.driver,
                    row.namespace.clone(),
                    row.latest().object.clone(),
                );
                let work = spawn_kube(cx, async move {
                    let release = crate::service::load(client, driver, namespace, object).await?;
                    Ok::<_, String>(preview::uninstall_plan(&release))
                });
                Plan::Loading(cx.spawn(async move |this, cx| {
                    let result = work.await;
                    this.update(cx, |this, cx| {
                        this.plan = match result {
                            Ok(plan) => Plan::Ready(plan),
                            Err(err) => Plan::Failed(err),
                        };
                        cx.notify();
                    })
                    .ok();
                }))
            }
            None => Plan::Failed("The cluster isn't connected.".into()),
        };
        Self {
            cluster,
            row,
            helm,
            plan,
            keep_history: false,
            no_hooks: false,
            wait: false,
            typed,
            running: None,
            error: None,
            focus: cx.focus_handle(),
            _subscriptions: subscriptions,
        }
    }

    fn typed_ok(&self, cx: &App) -> bool {
        !production(&self.cluster, cx) || self.typed.read(cx).value().trim() == self.row.name
    }

    fn run(&mut self, cx: &mut Context<Self>) {
        if self.running.is_some() || !matches!(self.plan, Plan::Ready(_)) {
            return;
        }
        if !self.typed_ok(cx) {
            self.error =
                Some("Type the release's name to uninstall it on a production cluster.".into());
            cx.notify();
            return;
        }
        if !guard(&self.cluster, cx) {
            return;
        }
        if let Some(changed) = super::release_changed(&self.cluster, &self.row, cx) {
            self.error = Some(changed);
            cx.notify();
            return;
        }
        let spec = UninstallSpec {
            name: self.row.name.clone(),
            namespace: self.row.namespace.clone(),
            driver: self.row.driver,
            keep_history: self.keep_history,
            no_hooks: self.no_hooks,
            wait: self.wait,
            timeout: crate::cli::settings(cx).timeout(),
        };
        let start = Start {
            cluster: self.cluster.clone(),
            kind: OpKind::Uninstall,
            namespace: spec.namespace.clone(),
            release: spec.name.clone(),
            helm: self.helm.clone(),
            target: crate::cli::target(&self.cluster, cx),
            invocation: cmd::uninstall(&spec),
        };
        self.running = crate::ops::start(start, cx);
        cx.notify();
    }

    fn render_plan(&self, plan: &UninstallPlan, cx: &mut Context<Self>) -> AnyElement {
        let colors = cx.colors().clone();
        let row =
            |icon: IconName, color: gpui::Hsla, kind: String, name: String, why: Option<String>| {
                h_flex()
                    .items_start()
                    .gap(u(8.0))
                    .py(u(4.0))
                    .border_b_1()
                    .border_color(colors.border_variant)
                    .text_size(u(12.0))
                    .child(
                        div()
                            .pt(u(1.0))
                            .child(Icon::new(icon).size(12.0).color(color)),
                    )
                    .child(
                        v_flex()
                            .flex_1()
                            .min_w_0()
                            .child(
                                h_flex()
                                    .gap(u(8.0))
                                    .child(
                                        div()
                                            .flex_none()
                                            .w(u(120.0))
                                            .truncate()
                                            .text_color(colors.text_dim)
                                            .child(kind),
                                    )
                                    .child(
                                        div()
                                            .flex_1()
                                            .min_w_0()
                                            .truncate()
                                            .font_family(fonts::MONO)
                                            .text_size(u(11.5))
                                            .child(name),
                                    ),
                            )
                            .when_some(why, |this, why| {
                                this.child(
                                    div()
                                        .text_size(u(11.5))
                                        .text_color(colors.text_dim)
                                        .child(why),
                                )
                            }),
                    )
                    .into_any_element()
            };
        let mut deleted: Vec<AnyElement> = plan
            .deleted
            .iter()
            .map(|key| {
                row(
                    IconName::Minus,
                    colors.red,
                    key.kind.clone(),
                    match &key.namespace {
                        Some(ns) => format!("{ns}/{}", key.name),
                        None => key.name.clone(),
                    },
                    None,
                )
            })
            .collect();
        if !self.keep_history {
            deleted.push(row(
                IconName::Minus,
                colors.red,
                "History".to_string(),
                format!(
                    "{} revisions ({}s)",
                    self.row.revisions.len(),
                    self.row.driver.label()
                ),
                None,
            ));
        }
        let kept: Vec<AnyElement> = if plan.kept.is_empty() {
            vec![widgets::muted(
                "Nothing: every object of the release is deleted.",
                &colors,
            )]
        } else {
            plan.kept
                .iter()
                .map(|(what, why)| {
                    let (kind, name) = what.split_once(' ').unwrap_or((what.as_str(), ""));
                    row(
                        IconName::Lock,
                        colors.yellow,
                        kind.to_string(),
                        name.to_string(),
                        Some(why.clone()),
                    )
                })
                .collect()
        };
        // `--no-hooks`: none of them runs.
        let hooks: Vec<AnyElement> = plan
            .hooks
            .iter()
            .filter(|_| !self.no_hooks)
            .map(|h| {
                widgets::note(
                    IconName::Zap,
                    colors.accent,
                    format!("{} {} runs {}", h.kind, h.name, h.events.join(", ")),
                    &colors,
                )
            })
            .collect();
        v_flex()
            .p(u(16.0))
            .gap(u(14.0))
            .child(
                h_flex()
                    .items_start()
                    .gap(u(18.0))
                    .child(
                        v_flex()
                            .id("uninstall-deleted")
                            .flex_1()
                            .min_w_0()
                            .max_h(u(300.0))
                            .overflow_y_scroll()
                            .child(widgets::dialog_title(
                                format!("Deleted · {} objects", plan.deleted.len()),
                                &colors,
                            ))
                            .children(deleted),
                    )
                    .child(
                        v_flex()
                            .id("uninstall-kept")
                            .flex_1()
                            .min_w_0()
                            .max_h(u(300.0))
                            .overflow_y_scroll()
                            .child(widgets::dialog_title("Stays", &colors))
                            .children(kept),
                    ),
            )
            .when(!hooks.is_empty(), |this| {
                this.child(
                    v_flex()
                        .gap(u(4.0))
                        .child(widgets::dialog_title("Hooks", &colors))
                        .children(hooks),
                )
            })
            .child(
                h_flex()
                    .gap(u(18.0))
                    .child(
                        checkbox(
                            "uninstall-keep-history",
                            self.keep_history,
                            "Keep history (--keep-history)",
                            &colors,
                        )
                        .on_click(cx.listener(|this, _, _, cx| {
                            this.keep_history = !this.keep_history;
                            cx.notify();
                        })),
                    )
                    .child(
                        checkbox(
                            "uninstall-no-hooks",
                            self.no_hooks,
                            "No hooks (--no-hooks)",
                            &colors,
                        )
                        .on_click(cx.listener(|this, _, _, cx| {
                            this.no_hooks = !this.no_hooks;
                            cx.notify();
                        })),
                    )
                    .child(
                        checkbox(
                            "uninstall-wait",
                            self.wait,
                            "Wait until deleted (--wait)",
                            &colors,
                        )
                        .on_click(cx.listener(|this, _, _, cx| {
                            this.wait = !this.wait;
                            cx.notify();
                        })),
                    ),
            )
            .when(production(&self.cluster, cx), |this| {
                this.child(super::typed_row(&self.row.name, &self.typed, &colors))
            })
            .into_any_element()
    }
}

impl Focusable for UninstallDialog {
    fn focus_handle(&self, _: &App) -> FocusHandle {
        self.focus.clone()
    }
}

impl Render for UninstallDialog {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let colors = cx.colors().clone();
        let prod = production(&self.cluster, cx);
        let lead = Icon::new(IconName::Trash)
            .size(16.0)
            .color(colors.red)
            .into_any_element();
        let mut extra = vec![widgets::muted(self.row.namespace.clone(), &colors)];
        if prod {
            extra.push(ProdBadge.into_any_element());
        }
        let body: AnyElement = match (self.running, &self.plan) {
            (Some(id), _) => super::operation(id, cx),
            (None, Plan::Loading(_)) => v_flex()
                .h(u(200.0))
                .items_center()
                .justify_center()
                .child(widgets::muted("Reading the release…", &colors))
                .into_any_element(),
            (None, Plan::Failed(err)) => v_flex()
                .h(u(200.0))
                .items_center()
                .justify_center()
                .child(widgets::note(
                    IconName::TriangleAlert,
                    colors.yellow,
                    err.clone(),
                    &colors,
                ))
                .into_any_element(),
            (None, Plan::Ready(plan)) => {
                let plan = plan.clone();
                self.render_plan(&plan, cx)
            }
        };
        let command = format!("helm uninstall {} -n {}", self.row.name, self.row.namespace);
        let note =
            error_line(&self.error, &colors).unwrap_or_else(|| widgets::muted(command, &colors));
        let buttons: Vec<AnyElement> = match self.running {
            Some(id) => {
                let done = HelmOps::global(cx)
                    .and_then(|ops| ops.read(cx).get(id).map(|op| !op.is_running()))
                    .unwrap_or(true);
                vec![super::close_button(if done { "Close" } else { "Hide" })]
            }
            None => vec![
                super::close_button("Cancel"),
                Button::new("uninstall-run")
                    .danger()
                    .icon(IconName::Trash)
                    .label("Uninstall")
                    .disabled(!matches!(self.plan, Plan::Ready(_)) || !self.typed_ok(cx))
                    .on_click(cx.listener(|this, _, _, cx| this.run(cx)))
                    .into_any_element(),
            ],
        };
        v_flex()
            .track_focus(&self.focus)
            .key_context("HelmDialog")
            .text_color(colors.text)
            .text_size(u(13.0))
            .child(header(
                lead,
                format!("Uninstall {}", self.row.name),
                extra,
                &colors,
            ))
            .child(body)
            .child(footer(Some(note), buttons, &colors))
    }
}
