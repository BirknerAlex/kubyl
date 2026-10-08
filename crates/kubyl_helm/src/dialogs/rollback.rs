//! Roll back a release (board 20): pick a revision from its history, see what changes from the
//! current revision to it (per-object manifest diff and values diff, from the stored revisions:
//! Helm has no rollback dry run that prints them), options, then `helm rollback`.

use std::collections::HashMap;
use std::sync::Arc;

use gpui::{
    AnyElement, App, AppContext as _, Context, Entity, FocusHandle, Focusable, IntoElement, Render,
    Subscription, Task, Window, div, prelude::*,
};
use gpui_component::input::{InputEvent, InputState};
use kubyl_core::{ClusterId, spawn_kube};
use kubyl_helm_core::cli::HelmInfo;
use kubyl_helm_core::cmd::{self, RollbackSpec};
use kubyl_helm_core::decode::{Release, Summary};
use kubyl_helm_core::preview;
use kubyl_ui::{ActiveColors, Button, Icon, IconName, ProdBadge, fonts, h_flex, u, v_flex};

use super::preview::{PreviewKind, PreviewPane};
use super::{
    checkbox, client, error_line, footer, guard, header, input_box, open, production, typed_input,
};
use crate::cli::HelmCli;
use crate::ops::{HelmOps, OpKind, Start};
use crate::releases::status_tone;
use crate::service::{ReleaseRow, Revision};
use crate::widgets;

/// Opens the rollback dialog (`to`: a revision to preselect).
pub fn open_rollback(
    cluster: ClusterId,
    row: ReleaseRow,
    to: Option<u32>,
    window: &mut Window,
    cx: &mut App,
) {
    if !guard(&cluster, cx) {
        return;
    }
    let Some(helm) = HelmCli::info(cx) else {
        super::open_missing(window, cx);
        return;
    };
    if row.revisions.len() < 2 {
        kubyl_core::NotificationCenter::push(
            cx,
            kubyl_core::Notification::info(format!(
                "{} has a single revision: there's nothing to roll back to.",
                row.name
            )),
        );
        return;
    }
    let view = cx.new(|cx| RollbackDialog::new(cluster, row, to, helm, window, cx));
    let focus = view.read(cx).focus.clone();
    open(view, 1100.0, Some(focus), window, cx);
}

/// The revision a rollback goes to by default: the newest one before the current that was
/// deployed (`superseded` or `deployed`), else the one before.
pub fn default_target(revisions: &[Revision]) -> Option<u32> {
    let current = revisions.first()?.revision;
    revisions
        .iter()
        .skip(1)
        .find(|r| matches!(r.status.as_str(), "superseded" | "deployed") && r.revision != current)
        .or_else(|| revisions.get(1))
        .map(|r| r.revision)
}

enum Diff {
    None,
    Loading(#[allow(dead_code)] Task<()>),
    Ready(Entity<PreviewPane>),
    Failed(String),
}

pub struct RollbackDialog {
    cluster: ClusterId,
    row: ReleaseRow,
    helm: HelmInfo,
    target: Option<u32>,
    summaries: HashMap<String, Summary>,
    /// Decoded revisions (values and manifests: in this dialog's memory only).
    releases: HashMap<u32, Arc<Release>>,
    diff: Diff,
    wait: bool,
    cleanup_on_fail: bool,
    no_hooks: bool,
    timeout: Entity<InputState>,
    typed: Entity<InputState>,
    running: Option<u64>,
    error: Option<String>,
    pub(crate) focus: FocusHandle,
    _tasks: Vec<Task<()>>,
    _subscriptions: Vec<Subscription>,
}

impl RollbackDialog {
    fn new(
        cluster: ClusterId,
        row: ReleaseRow,
        to: Option<u32>,
        helm: HelmInfo,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Self {
        let settings = crate::cli::settings(cx);
        let timeout = cx.new(|cx| {
            let mut state = InputState::new(window, cx).placeholder("5m0s");
            state.set_value(settings.timeout(), window, cx);
            state
        });
        let typed = typed_input(row.name.clone(), window, cx);
        let mut subscriptions =
            vec![
                cx.subscribe(&typed, |this: &mut Self, _, event: &InputEvent, cx| {
                    if matches!(event, InputEvent::Change) {
                        this.error = None;
                        cx.notify();
                    }
                }),
            ];
        if let Some(ops) = HelmOps::global(cx) {
            subscriptions.push(cx.observe(&ops, |_, _, cx| cx.notify()));
        }
        let target = to.or_else(|| default_target(&row.revisions));
        let mut this = Self {
            cluster,
            row,
            helm,
            target: None,
            summaries: HashMap::new(),
            releases: HashMap::new(),
            diff: Diff::None,
            wait: false,
            cleanup_on_fail: true,
            no_hooks: false,
            timeout,
            typed,
            running: None,
            error: None,
            focus: cx.focus_handle(),
            _tasks: Vec::new(),
            _subscriptions: subscriptions,
        };
        this.load_summaries(cx);
        if let Some(target) = target {
            this.pick(target, cx);
        }
        this
    }

    /// The descriptions of the revisions (what each was: install, upgrade, rollback…).
    fn load_summaries(&mut self, cx: &mut Context<Self>) {
        let Some(client) = client(&self.cluster, cx) else {
            return;
        };
        let objects: Vec<String> = self
            .row
            .revisions
            .iter()
            .take(20)
            .map(|r| r.object.clone())
            .collect();
        let work = spawn_kube(
            cx,
            crate::service::load_summaries(
                client,
                self.row.driver,
                self.row.namespace.clone(),
                objects,
            ),
        );
        self._tasks.push(cx.spawn(async move |this, cx| {
            let results = work.await;
            this.update(cx, |this, cx| {
                for (object, summary) in results {
                    if let Ok(summary) = summary {
                        this.summaries.insert(object, summary);
                    }
                }
                cx.notify();
            })
            .ok();
        }));
    }

    fn pick(&mut self, revision: u32, cx: &mut Context<Self>) {
        self.target = Some(revision);
        self.error = None;
        let current = self.row.latest().clone();
        let Some(target) = self
            .row
            .revisions
            .iter()
            .find(|r| r.revision == revision)
            .cloned()
        else {
            return;
        };
        let Some(client) = client(&self.cluster, cx) else {
            self.diff = Diff::Failed("The cluster isn't connected.".into());
            return;
        };
        let (driver, namespace) = (self.row.driver, self.row.namespace.clone());
        let cached_current = self.releases.get(&current.revision).cloned();
        let cached_target = self.releases.get(&revision).cloned();
        let work = spawn_kube(cx, async move {
            let current = match cached_current {
                Some(r) => r,
                None => Arc::new(
                    crate::service::load(client.clone(), driver, namespace.clone(), current.object)
                        .await?,
                ),
            };
            let target = match cached_target {
                Some(r) => r,
                None => {
                    Arc::new(crate::service::load(client, driver, namespace, target.object).await?)
                }
            };
            let preview = preview::rollback_preview(&current, &target);
            Ok::<_, String>((current, target, preview))
        });
        let current_revision = self.row.latest().revision;
        let task = cx.spawn(async move |this, cx| {
            let result = work.await;
            this.update(cx, |this, cx| {
                if this.target != Some(revision) {
                    return;
                }
                match result {
                    Ok((current, target, preview)) => {
                        this.releases.insert(current_revision, current);
                        this.releases.insert(revision, target);
                        let no_hooks = this.no_hooks;
                        let pane = cx.new(|cx| {
                            let mut pane =
                                PreviewPane::new(PreviewKind::Rollback, Arc::new(preview));
                            pane.set_hooks_skipped(no_hooks, cx);
                            pane
                        });
                        this.diff = Diff::Ready(pane);
                    }
                    Err(err) => this.diff = Diff::Failed(err),
                }
                cx.notify();
            })
            .ok();
        });
        self.diff = Diff::Loading(task);
        cx.notify();
    }

    fn typed_ok(&self, cx: &App) -> bool {
        !production(&self.cluster, cx) || self.typed.read(cx).value().trim() == self.row.name
    }

    fn run(&mut self, cx: &mut Context<Self>) {
        let Some(revision) = self.target else {
            return;
        };
        if !self.typed_ok(cx) {
            self.error =
                Some("Type the release's name to roll it back on a production cluster.".into());
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
        let timeout = self.timeout.read(cx).value().trim().to_string();
        if !timeout.is_empty() && cmd::parse_duration(&timeout).is_none() {
            self.error = Some("The timeout is a duration like 5m0s or 90s.".into());
            cx.notify();
            return;
        }
        let spec = RollbackSpec {
            name: self.row.name.clone(),
            namespace: self.row.namespace.clone(),
            driver: self.row.driver,
            revision,
            wait: self.wait,
            timeout,
            cleanup_on_fail: self.cleanup_on_fail,
            no_hooks: self.no_hooks,
        };
        let start = Start {
            cluster: self.cluster.clone(),
            kind: OpKind::Rollback,
            namespace: spec.namespace.clone(),
            release: spec.name.clone(),
            helm: self.helm.clone(),
            target: crate::cli::target(&self.cluster, cx),
            invocation: cmd::rollback(&spec),
        };
        self.running = crate::ops::start(start, cx);
        cx.notify();
    }

    fn render_revisions(&self, cx: &mut Context<Self>) -> AnyElement {
        let colors = cx.colors().clone();
        let current = self.row.latest().revision;
        let mut list = v_flex().gap(u(2.0));
        for (i, revision) in self.row.revisions.iter().enumerate().take(20) {
            let on = self.target == Some(revision.revision);
            let is_current = revision.revision == current;
            let summary = self.summaries.get(&revision.object);
            let detail = summary
                .map(|s| {
                    format!(
                        "{} · {}",
                        s.description.clone().unwrap_or_default(),
                        s.chart_label()
                    )
                })
                .unwrap_or_default();
            let number = revision.revision;
            list = list.child(
                h_flex()
                    .id(("rollback-revision", i))
                    .gap(u(8.0))
                    .px(u(10.0))
                    .py(u(6.0))
                    .rounded(u(6.0))
                    .text_size(u(12.0))
                    .when(on, |this| {
                        this.bg(colors.selection)
                            .border_1()
                            .border_color(colors.accent)
                    })
                    .when(!on && !is_current, |this| {
                        this.cursor_pointer().hover(|s| s.bg(colors.hover))
                    })
                    .when(is_current, |this| this.opacity(0.7))
                    .child(
                        div()
                            .flex_none()
                            .size(u(12.0))
                            .rounded_full()
                            .border_1()
                            .border_color(if on { colors.accent } else { colors.text_faint })
                            .flex()
                            .items_center()
                            .justify_center()
                            .when(on, |this| {
                                this.child(div().size(u(6.0)).rounded_full().bg(colors.accent))
                            }),
                    )
                    .child(
                        div()
                            .flex_none()
                            .w(u(24.0))
                            .font_family(fonts::MONO)
                            .child(number.to_string()),
                    )
                    .child(div().flex_none().w(u(118.0)).child(widgets::pill(
                        revision.status.clone(),
                        status_tone(&revision.status),
                        &colors,
                    )))
                    .child(
                        div()
                            .flex_1()
                            .min_w_0()
                            .truncate()
                            .text_color(colors.text_dim)
                            .child(if is_current {
                                format!("current · {detail}")
                            } else {
                                detail
                            }),
                    )
                    .child(
                        div()
                            .flex_none()
                            .font_family(fonts::MONO)
                            .text_color(colors.text_dim)
                            .child(widgets::ago(revision.modified)),
                    )
                    .when(!is_current, |this| {
                        this.on_click(cx.listener(move |this, _, _, cx| this.pick(number, cx)))
                    }),
            );
        }
        let options = v_flex()
            .gap(u(7.0))
            .child(widgets::dialog_title("Options", &colors))
            .child(
                checkbox(
                    "rollback-wait",
                    self.wait,
                    "Wait until ready (--wait)",
                    &colors,
                )
                .on_click(cx.listener(|this, _, _, cx| {
                    this.wait = !this.wait;
                    cx.notify();
                })),
            )
            .child(
                checkbox(
                    "rollback-cleanup",
                    self.cleanup_on_fail,
                    "Clean up on failure (--cleanup-on-fail)",
                    &colors,
                )
                .on_click(cx.listener(|this, _, _, cx| {
                    this.cleanup_on_fail = !this.cleanup_on_fail;
                    cx.notify();
                })),
            )
            .child(
                checkbox(
                    "rollback-no-hooks",
                    self.no_hooks,
                    "No hooks (--no-hooks)",
                    &colors,
                )
                .on_click(cx.listener(|this, _, _, cx| {
                    this.no_hooks = !this.no_hooks;
                    if let Diff::Ready(pane) = &this.diff {
                        let skipped = this.no_hooks;
                        pane.update(cx, |pane, cx| pane.set_hooks_skipped(skipped, cx));
                    }
                    cx.notify();
                })),
            )
            .child(
                h_flex()
                    .gap(u(8.0))
                    .text_size(u(12.5))
                    .child("Timeout")
                    .child(
                        div()
                            .w(u(90.0))
                            .child(input_box(&self.timeout, true, &colors)),
                    ),
            );
        v_flex()
            .flex_none()
            .w(u(360.0))
            .h_full()
            .p(u(12.0))
            .gap(u(6.0))
            .border_r_1()
            .border_color(colors.border_variant)
            .child(widgets::dialog_title("Roll back to", &colors))
            .child(
                div()
                    .id("rollback-revisions")
                    .flex_1()
                    .min_h_0()
                    .overflow_y_scroll()
                    .child(list),
            )
            .child(options)
            .into_any_element()
    }
}

impl Focusable for RollbackDialog {
    fn focus_handle(&self, _: &App) -> FocusHandle {
        self.focus.clone()
    }
}

impl Render for RollbackDialog {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let colors = cx.colors().clone();
        let prod = production(&self.cluster, cx);
        let lead = Icon::new(IconName::RotateCcw)
            .size(16.0)
            .color(colors.accent)
            .into_any_element();
        let mut extra = vec![widgets::muted(self.row.namespace.clone(), &colors)];
        if prod {
            extra.push(ProdBadge.into_any_element());
        }
        let title = format!("Roll back {}", self.row.name);
        let body: AnyElement = if let Some(id) = self.running {
            super::operation(id, cx)
        } else {
            let diff: AnyElement = match &self.diff {
                Diff::None => widgets::empty("Pick a revision.", &colors),
                Diff::Loading(_) => widgets::empty("Reading both revisions…", &colors),
                Diff::Failed(err) => widgets::empty(err.clone(), &colors),
                Diff::Ready(pane) => pane.clone().into_any_element(),
            };
            h_flex()
                .h(u(470.0))
                .items_start()
                .child(self.render_revisions(cx))
                .child(div().flex_1().min_w_0().h_full().child(diff))
                .into_any_element()
        };
        let current = self.row.latest().revision;
        let note = error_line(&self.error, &colors).unwrap_or_else(|| {
            widgets::muted(
                match self.target {
                    Some(target) => format!(
                        "Creates revision {} with revision {target}'s manifest and values",
                        current + 1
                    ),
                    None => String::new(),
                },
                &colors,
            )
        });
        let buttons: Vec<AnyElement> = match self.running {
            Some(id) => {
                let done = HelmOps::global(cx)
                    .and_then(|ops| ops.read(cx).get(id).map(|op| !op.is_running()))
                    .unwrap_or(true);
                vec![super::close_button(if done { "Close" } else { "Hide" })]
            }
            None => {
                let mut buttons = Vec::new();
                if prod {
                    buttons.push(
                        h_flex()
                            .gap(u(8.0))
                            .text_size(u(12.0))
                            .text_color(colors.text_muted)
                            .child(format!("Type {}", self.row.name))
                            .child(
                                div()
                                    .w(u(170.0))
                                    .child(input_box(&self.typed, true, &colors)),
                            )
                            .into_any_element(),
                    );
                }
                buttons.push(super::close_button("Cancel"));
                buttons.push(
                    Button::new("rollback-run")
                        .primary()
                        .icon(IconName::RotateCcw)
                        .label(match self.target {
                            Some(t) => format!("Roll back to {t}"),
                            None => "Roll back".into(),
                        })
                        .disabled(
                            self.target.is_none()
                                || !matches!(self.diff, Diff::Ready(_))
                                || !self.typed_ok(cx),
                        )
                        .on_click(cx.listener(|this, _, _, cx| this.run(cx)))
                        .into_any_element(),
                );
                buttons
            }
        };
        v_flex()
            .track_focus(&self.focus)
            .key_context("HelmDialog")
            .text_color(colors.text)
            .text_size(u(13.0))
            .child(header(lead, title, extra, &colors))
            .child(body)
            .child(footer(Some(note), buttons, &colors))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn revision(revision: u32, status: &str) -> Revision {
        Revision {
            revision,
            status: status.into(),
            modified: None,
            object: format!("sh.helm.release.v1.web.v{revision}"),
        }
    }

    #[test]
    fn rollbacks_default_to_the_last_good_revision() {
        let revisions = vec![
            revision(5, "failed"),
            revision(4, "failed"),
            revision(3, "superseded"),
            revision(2, "superseded"),
        ];
        assert_eq!(default_target(&revisions), Some(3));
        assert_eq!(
            default_target(&[revision(2, "pending-upgrade"), revision(1, "deployed")]),
            Some(1)
        );
        assert_eq!(default_target(&[revision(1, "deployed")]), None);
    }
}
