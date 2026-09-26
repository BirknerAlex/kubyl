//! The summary every write shows first (board 8 · Update confirmation): what changes, from →
//! to, the checks that warn or fail, each risk to accept, that updates can't be undone, and the
//! typed cluster name on PROD.

use gpui::{
    AnyElement, App, AppContext as _, Context, Entity, FocusHandle, Focusable, FontWeight,
    IntoElement, Render, SharedString, Subscription, Window, div, prelude::*, px,
};
use gpui_component::WindowExt as _;
use gpui_component::input::{Input, InputEvent, InputState};
use kubyl_core::{ClusterId, Notification, NotificationCenter};
use kubyl_kube::ConnectionManager;
use kubyl_ui::{ActiveColors, Button, Colors, Icon, IconName, ProdBadge, fonts, h_flex, u, v_flex};

use crate::check::{Check, CheckStatus, Counts};
use crate::model::{Plan, ProviderKind, Scope};
use crate::service::Updates;
use crate::view::widgets::{kv, status_icon};

/// Opens the confirmation of `plan`. `checks`: the pre-flight results of its target, if run.
pub fn open(
    cluster: ClusterId,
    provider: ProviderKind,
    plan: Plan,
    checks: Option<Vec<Check>>,
    window: &mut Window,
    cx: &mut App,
) {
    let read_only =
        ConnectionManager::try_global(cx).is_some_and(|m| m.read(cx).caps(&cluster).read_only);
    if read_only {
        NotificationCenter::push(
            cx,
            Notification::error("This cluster is read-only in Kubyl."),
        );
        return;
    }
    let view = cx.new(|cx| ConfirmDialog::new(cluster, provider, plan, checks, window, cx));
    let colors = cx.colors().clone();
    let focus = view.read(cx).initial_focus(cx);
    window.open_dialog(cx, move |dialog, _, _| {
        dialog
            .w(px(580.0))
            .margin_top(px(60.0))
            .p_0()
            .bg(colors.panel)
            .close_button(false)
            // Enter in the input would close the dialog first; the view submits itself.
            .on_ok(|_, _, _| false)
            .content({
                let view = view.clone();
                move |content, _, _| content.child(view.clone())
            })
    });
    window.focus(&focus, cx);
}

pub struct ConfirmDialog {
    cluster: ClusterId,
    provider: ProviderKind,
    plan: Plan,
    checks: Option<Vec<Check>>,
    /// "I've read the pre-flight results" (needed when a check failed).
    read_ack: bool,
    /// One per risk of a conditional update.
    accepted: Vec<bool>,
    typed: Entity<InputState>,
    error: Option<String>,
    focus: FocusHandle,
    _subscriptions: Vec<Subscription>,
}

impl ConfirmDialog {
    fn new(
        cluster: ClusterId,
        provider: ProviderKind,
        plan: Plan,
        checks: Option<Vec<Check>>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Self {
        let name = cluster_name(&cluster, cx);
        let typed = cx.new(|cx| InputState::new(window, cx).placeholder(name));
        let subscriptions =
            vec![
                cx.subscribe_in(&typed, window, |this, _, event: &InputEvent, window, cx| {
                    match event {
                        InputEvent::Change => {
                            this.error = None;
                            cx.notify();
                        }
                        InputEvent::PressEnter { .. } => this.submit(window, cx),
                        _ => {}
                    }
                }),
            ];
        let accepted = vec![false; plan.risks.len()];
        Self {
            cluster,
            provider,
            plan,
            checks,
            read_ack: false,
            accepted,
            typed,
            error: None,
            focus: cx.focus_handle(),
            _subscriptions: subscriptions,
        }
    }

    fn initial_focus(&self, cx: &App) -> FocusHandle {
        if production(&self.cluster, cx) {
            self.typed.read(cx).focus_handle(cx)
        } else {
            self.focus.clone()
        }
    }

    fn failed(&self) -> bool {
        self.checks
            .as_ref()
            .is_some_and(|c| c.iter().any(|c| c.status == CheckStatus::Fail))
    }

    fn typed_ok(&self, cx: &App) -> bool {
        !production(&self.cluster, cx)
            || self.typed.read(cx).value().trim() == cluster_name(&self.cluster, cx)
    }

    /// What still blocks the button.
    fn missing(&self, cx: &App) -> Option<&'static str> {
        if self.failed() && !self.read_ack {
            Some("Confirm that you read the pre-flight results.")
        } else if self.accepted.iter().any(|a| !a) {
            Some("Accept each risk first.")
        } else if !self.typed_ok(cx) {
            Some("Type the cluster's name to confirm.")
        } else {
            None
        }
    }

    fn submit(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if let Some(missing) = self.missing(cx) {
            self.error = Some(missing.into());
            cx.notify();
            return;
        }
        let Some(updates) = Updates::global(cx) else {
            return;
        };
        let plan = self.plan.clone();
        updates.update(cx, |updates, cx| updates.start(&self.cluster, plan, cx));
        window.close_dialog(cx);
    }

    fn checks_block(&self, colors: &Colors) -> AnyElement {
        let Some(checks) = &self.checks else {
            return div()
                .text_color(colors.text_dim)
                .child("not run for this target")
                .into_any_element();
        };
        let counts = Counts::of(checks);
        let mut block = v_flex().gap(u(4.0)).child(
            h_flex()
                .gap(u(6.0))
                .when(counts.failed > 0, |this| {
                    this.child(
                        div()
                            .text_color(colors.red)
                            .child(format!("{} failed", counts.failed)),
                    )
                })
                .when(counts.warnings > 0, |this| {
                    this.child(
                        div()
                            .text_color(colors.yellow)
                            .child(match counts.warnings {
                                1 => "1 warning".to_string(),
                                n => format!("{n} warnings"),
                            }),
                    )
                })
                .when(counts.unknown > 0, |this| {
                    this.child(
                        div()
                            .text_color(colors.text_dim)
                            .child(format!("{} not checked", counts.unknown)),
                    )
                })
                .when(counts.passed > 0, |this| {
                    this.child(
                        div()
                            .text_color(colors.green)
                            .child(format!("{} passed", counts.passed)),
                    )
                }),
        );
        for check in checks
            .iter()
            .filter(|c| matches!(c.status, CheckStatus::Fail | CheckStatus::Warn))
        {
            let (icon, color) = status_icon(check.status, colors);
            block = block.child(
                h_flex()
                    .gap(u(6.0))
                    .text_size(u(12.0))
                    .child(Icon::new(icon).size(12.0).color(color))
                    .child(div().min_w_0().truncate().child(check.title.clone())),
            );
        }
        block.into_any_element()
    }
}

fn production(cluster: &ClusterId, cx: &App) -> bool {
    ConnectionManager::try_global(cx).is_some_and(|m| m.read(cx).caps(cluster).production)
}

fn cluster_name(cluster: &ClusterId, cx: &App) -> String {
    ConnectionManager::try_global(cx)
        .map(|m| m.read(cx).display_name(cluster).to_string())
        .unwrap_or_else(|| cluster.to_string())
}

fn checkbox(id: impl Into<gpui::ElementId>, on: bool, colors: &Colors) -> impl IntoElement {
    div()
        .id(id)
        .flex_none()
        .mt(u(2.0))
        .size(u(14.0))
        .rounded(u(3.0))
        .flex()
        .items_center()
        .justify_center()
        .map(|this| {
            if on {
                this.bg(colors.accent).child(
                    Icon::new(IconName::Check)
                        .size(10.0)
                        .color(colors.on_accent),
                )
            } else {
                this.border_1().border_color(colors.text_faint)
            }
        })
}

/// What can't be undone, per provider.
fn undo_text(provider: ProviderKind) -> &'static str {
    match provider {
        ProviderKind::OpenShift => {
            "OpenShift doesn't roll back a cluster; a failed update is fixed forward."
        }
        ProviderKind::Eks | ProviderKind::Gke | ProviderKind::Aks => {
            "A control plane can't be downgraded; node pools can only move forward with it."
        }
        _ => {
            "Kubernetes doesn't support downgrading a control plane; nodes are replaced or updated in place."
        }
    }
}

impl Focusable for ConfirmDialog {
    fn focus_handle(&self, _: &App) -> FocusHandle {
        self.focus.clone()
    }
}

impl Render for ConfirmDialog {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let colors = cx.colors().clone();
        let prod = production(&self.cluster, cx);
        let name = cluster_name(&self.cluster, cx);
        let plan = self.plan.clone();
        let channel = matches!(plan.scope, Scope::Channel(_));
        let busy = Updates::global(cx).is_some_and(|u| u.read(cx).busy(&self.cluster).is_some());
        let label_width = 104.0;
        let mut body = v_flex()
            .p(u(16.0))
            .gap(u(10.0))
            .child(kv(
                "Cluster",
                div()
                    .font_family(fonts::MONO)
                    .text_size(u(12.0))
                    .child(name.clone()),
                label_width,
                &colors,
            ))
            .child(kv(
                "Provider",
                div().child(self.provider.label()),
                label_width,
                &colors,
            ))
            .child(kv(
                "From → To",
                h_flex()
                    .gap(u(6.0))
                    .flex_wrap()
                    .child(
                        div()
                            .font_family(fonts::MONO)
                            .text_size(u(12.0))
                            .child(format!("{} →", plan.from)),
                    )
                    .child(
                        div()
                            .font_family(fonts::MONO)
                            .text_size(u(12.0))
                            .text_color(colors.accent)
                            .child(plan.to.clone()),
                    )
                    .child(
                        div()
                            .text_color(colors.text_dim)
                            .child(format!("({})", plan.kind_label)),
                    ),
                label_width,
                &colors,
            ))
            .child(kv(
                "What changes",
                v_flex().gap(u(3.0)).children(plan.changes.iter().map(|c| {
                    div()
                        .font_family(fonts::MONO)
                        .text_size(u(11.5))
                        .child(c.clone())
                })),
                label_width,
                &colors,
            ));
        if !channel {
            body = body.child(kv(
                "Checks",
                self.checks_block(&colors),
                label_width,
                &colors,
            ));
        }
        for note in &plan.notes {
            body = body.child(
                h_flex()
                    .items_start()
                    .gap(u(8.0))
                    .text_size(u(12.0))
                    .text_color(colors.text_muted)
                    .child(Icon::new(IconName::Info).size(12.0).color(colors.text_dim))
                    .child(div().flex_1().child(note.clone())),
            );
        }
        // Each risk is its own explicit step.
        for (ix, risk) in plan.risks.iter().enumerate() {
            let on = self.accepted[ix];
            let url = risk.url.clone();
            body = body.child(
                h_flex()
                    .id(("risk", ix))
                    .items_start()
                    .gap(u(8.0))
                    .p(u(10.0))
                    .rounded(u(6.0))
                    .border_1()
                    .border_color(colors.yellow.opacity(0.5))
                    .bg(colors.yellow.opacity(0.06))
                    .cursor_pointer()
                    .on_click(cx.listener(move |this, _, _, cx| {
                        this.accepted[ix] = !this.accepted[ix];
                        this.error = None;
                        cx.notify();
                    }))
                    .child(checkbox(("risk-box", ix), on, &colors))
                    .child(
                        v_flex()
                            .flex_1()
                            .gap(u(3.0))
                            .text_size(u(12.0))
                            .child(
                                h_flex().gap(u(5.0)).child("I accept the risk").child(
                                    div()
                                        .font_family(fonts::MONO)
                                        .text_color(colors.yellow)
                                        .child(risk.name.clone()),
                                ),
                            )
                            .child(
                                div()
                                    .text_color(colors.text_muted)
                                    .child(risk.message.clone()),
                            )
                            .when_some(url, |this, url| {
                                this.child(
                                    div()
                                        .id(("risk-url", ix))
                                        .text_color(colors.accent)
                                        .cursor_pointer()
                                        .hover(|s| s.underline())
                                        .child("Learn more")
                                        .on_click(move |_, window, cx| {
                                            crate::view::widgets::open_url(&url, window, cx)
                                        }),
                                )
                            }),
                    ),
            );
        }
        if plan.irreversible {
            body = body.child(
                h_flex()
                    .items_start()
                    .gap(u(8.0))
                    .p(u(10.0))
                    .rounded(u(6.0))
                    .border_1()
                    .border_color(colors.red.opacity(0.6))
                    .bg(colors.red.opacity(0.08))
                    .text_size(u(12.5))
                    .child(
                        Icon::new(IconName::TriangleAlert)
                            .size(14.0)
                            .color(colors.red),
                    )
                    .child(
                        div()
                            .flex_1()
                            .child(
                                div()
                                    .font_weight(FontWeight::MEDIUM)
                                    .text_color(colors.red)
                                    .child("Updates can't be undone."),
                            )
                            .child(undo_text(self.provider)),
                    ),
            );
        }
        if self.failed() {
            let on = self.read_ack;
            body = body.child(
                h_flex()
                    .id("read-ack")
                    .items_start()
                    .gap(u(8.0))
                    .cursor_pointer()
                    .on_click(cx.listener(|this, _, _, cx| {
                        this.read_ack = !this.read_ack;
                        this.error = None;
                        cx.notify();
                    }))
                    .child(checkbox("read-ack-box", on, &colors))
                    .child(
                        v_flex()
                            .text_size(u(12.5))
                            .child("I've read the pre-flight results")
                            .child(
                                div()
                                    .text_size(u(11.5))
                                    .text_color(colors.text_dim)
                                    .child("Required because a check failed"),
                            ),
                    ),
            );
        }
        if prod {
            body = body.child(
                v_flex()
                    .gap(u(6.0))
                    .child(
                        h_flex()
                            .gap(u(5.0))
                            .text_size(u(12.0))
                            .text_color(colors.text_muted)
                            .child("Type")
                            .child(
                                div()
                                    .font_family(fonts::MONO)
                                    .text_color(colors.text)
                                    .child(name),
                            )
                            .child("to confirm"),
                    )
                    .child(
                        div()
                            .key_context("UpdatesConfirm")
                            .h(u(30.0))
                            .px(u(8.0))
                            .flex()
                            .items_center()
                            .rounded(u(5.0))
                            .border_1()
                            .border_color(colors.border)
                            .bg(colors.input_background)
                            .font_family(fonts::MONO)
                            .child(Input::new(&self.typed).appearance(false).text_size(u(12.5))),
                    ),
            );
        }
        if let Some(error) = &self.error {
            body = body.child(
                div()
                    .text_size(u(12.0))
                    .text_color(colors.red)
                    .child(error.clone()),
            );
        }
        let ready = self.missing(cx).is_none() && !busy;
        let confirm_label: SharedString = if channel {
            "Change channel".into()
        } else {
            "Start update".into()
        };
        v_flex()
            .track_focus(&self.focus)
            .text_color(colors.text)
            .text_size(u(13.0))
            .child(
                h_flex()
                    .gap(u(10.0))
                    .px(u(16.0))
                    .py(u(14.0))
                    .border_b_1()
                    .border_color(colors.border_variant)
                    .child(Icon::new(IconName::ArrowUp).size(16.0).color(colors.accent))
                    .child(
                        div()
                            .flex_1()
                            .min_w_0()
                            .truncate()
                            .font_weight(FontWeight::SEMIBOLD)
                            .child(plan.title.clone()),
                    )
                    .when(prod, |this| this.child(ProdBadge)),
            )
            .child(body)
            .child(
                h_flex()
                    .gap(u(8.0))
                    .px(u(16.0))
                    .py(u(12.0))
                    .border_t_1()
                    .border_color(colors.border_variant)
                    .child(
                        div()
                            .text_size(u(11.5))
                            .text_color(colors.text_dim)
                            .child(if channel {
                                "Nothing is updated until you start an update."
                            } else {
                                "The cluster runs the update; Kubyl tracks it."
                            }),
                    )
                    .child(div().flex_1())
                    .child(
                        Button::new("updates-confirm-cancel")
                            .ghost()
                            .label("Cancel")
                            .on_click(|_, window, cx| window.close_dialog(cx)),
                    )
                    .child({
                        let button = Button::new("updates-confirm-start")
                            .label(confirm_label)
                            .icon(IconName::ArrowUp)
                            .disabled(!ready)
                            .on_click(cx.listener(|this, _, window, cx| this.submit(window, cx)));
                        if channel {
                            button.primary()
                        } else {
                            button.danger()
                        }
                    }),
            )
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::check::Check;
    use crate::model::Scope;
    use gpui::TestAppContext;

    /// A failed check needs "I've read the results", and each risk its own acceptance, before
    /// the update can start.
    #[gpui::test]
    fn failed_checks_and_risks_gate_the_button(cx: &mut TestAppContext) {
        let _updates = crate::view::tests::setup(cx);
        let status =
            crate::openshift::parse_cluster_version(&crate::view::tests::cluster_version());
        let plan = crate::openshift::plan(&status, &Scope::ControlPlane, "4.17.13", "ocp").unwrap();
        assert_eq!(plan.risks.len(), 1);
        let checks = vec![Check::new("pdb", "PDB", CheckStatus::Fail, "blocks")];
        let slot: std::rc::Rc<std::cell::RefCell<Option<Entity<ConfirmDialog>>>> =
            Default::default();
        let (_root, cx) = cx.add_window_view({
            let slot = slot.clone();
            move |window, cx| {
                let view = cx.new(|cx| {
                    ConfirmDialog::new(
                        ClusterId::new("ocp"),
                        ProviderKind::OpenShift,
                        plan,
                        Some(checks),
                        window,
                        cx,
                    )
                });
                *slot.borrow_mut() = Some(view.clone());
                gpui_component::Root::new(view, window, cx)
            }
        });
        let dialog = slot.borrow().clone().unwrap();
        cx.run_until_parked();
        dialog.update_in(cx, |dialog, _, cx| {
            assert!(dialog.plan.irreversible);
            assert_eq!(
                dialog.missing(cx),
                Some("Confirm that you read the pre-flight results.")
            );
            dialog.read_ack = true;
            assert_eq!(dialog.missing(cx), Some("Accept each risk first."));
            dialog.accepted[0] = true;
            // Not a production cluster: no typed name.
            assert_eq!(dialog.missing(cx), None);
        });
    }
}
