//! Upgrade a release (board 20): the chart version (from the release's repository when Kubyl can
//! tell, else a typed reference), the values (the current user-supplied ones in the editor; or
//! reuse them, or reset to the chart's defaults), options; then the review: a per-object diff of
//! the dry run against the current manifest, the values diff (masked), CRDs Helm won't apply,
//! hooks; then `helm upgrade`.

use std::sync::Arc;

use gpui::{
    AnyElement, App, AppContext as _, Context, Entity, FocusHandle, Focusable, IntoElement, Render,
    Subscription, Task, Window, div, prelude::*, px,
};
use gpui_component::WindowExt as _;
use gpui_component::button::Button as MenuButton;
use gpui_component::input::{InputEvent, InputState};
use gpui_component::menu::{DropdownMenu as _, PopupMenuItem};
use kubyl_core::actions::OpenView;
use kubyl_core::{ClusterId, ViewKind, ViewRequest, spawn_kube};
use kubyl_helm_core::cli::{self, HelmInfo};
use kubyl_helm_core::cmd::{self, Mode, UpgradeSpec, ValuesMode, WriteOptions};
use kubyl_helm_core::decode::Release;
use kubyl_helm_core::preview;
use kubyl_helm_core::repo::{self, ChartRef};
use kubyl_ui::{ActiveColors, Button, Icon, IconName, ProdBadge, fonts, h_flex, u, v_flex};

use super::preview::{PreviewKind, PreviewPane};
use super::{
    checkbox, client, error_line, footer, form_row, guard, header, input_box, open, production,
    typed_input,
};
use crate::cli::HelmCli;
use crate::ops::{HelmOps, OpKind, OpState, Start};
use crate::service::ReleaseRow;
use crate::values_editor::ValuesEditor;
use crate::widgets;

/// Opens the upgrade dialog for a release.
pub fn open_upgrade(cluster: ClusterId, row: ReleaseRow, window: &mut Window, cx: &mut App) {
    if !guard(&cluster, cx) {
        return;
    }
    let Some(helm) = HelmCli::info(cx) else {
        super::open_missing(window, cx);
        return;
    };
    let view = cx.new(|cx| UpgradeDialog::new(cluster, row, helm, window, cx));
    let focus = view.read(cx).focus.clone();
    open(view, 1080.0, Some(focus), window, cx);
}

/// Where the values start from.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Values {
    /// The current user-supplied values, edited (`--reset-values -f -`).
    Edit,
    /// The last release's values, with the editor's merged on top (`--reuse-values -f -`).
    Reuse,
    /// The chart's defaults only (`--reset-values`, the editor's values on top).
    Reset,
}

enum Step {
    Loading(#[allow(dead_code)] Task<()>),
    Failed(String),
    Configure,
    Previewing(#[allow(dead_code)] Task<()>),
    /// The review, and exactly what its dry run ran: Upgrade applies that (with the chart
    /// version the dry run rendered), never a command rebuilt from the form.
    Preview {
        pane: Entity<PreviewPane>,
        spec: Box<UpgradeSpec>,
        values: String,
    },
    Running(u64),
}

pub struct UpgradeDialog {
    cluster: ClusterId,
    row: ReleaseRow,
    helm: HelmInfo,
    current: Option<Arc<Release>>,
    /// Charts of that name in the added repositories.
    candidates: Vec<ChartRef>,
    chart: Option<ChartRef>,
    chart_input: Entity<InputState>,
    versions: Vec<(String, Option<String>)>,
    version: Option<String>,
    version_input: Entity<InputState>,
    timeout: Entity<InputState>,
    description: Entity<InputState>,
    typed: Entity<InputState>,
    wait: bool,
    atomic: bool,
    values_mode: Values,
    values: Entity<ValuesEditor>,
    step: Step,
    error: Option<String>,
    pub(crate) focus: FocusHandle,
    /// The chart's versions and the picked version's details: a new load replaces (and so
    /// cancels) the one before, so an older answer never lands after a newer pick.
    _versions_task: Option<Task<()>>,
    _details_task: Option<Task<()>>,
    _subscriptions: Vec<Subscription>,
}

impl UpgradeDialog {
    fn new(
        cluster: ClusterId,
        row: ReleaseRow,
        helm: HelmInfo,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Self {
        let settings = crate::cli::settings(cx);
        let make = |placeholder: &str,
                    value: Option<String>,
                    window: &mut Window,
                    cx: &mut Context<Self>| {
            let placeholder = placeholder.to_string();
            cx.new(|cx| {
                let mut state = InputState::new(window, cx).placeholder(placeholder);
                if let Some(value) = value {
                    state.set_value(value, window, cx);
                }
                state
            })
        };
        let chart_input = make("repo/chart, or oci://registry/path/chart", None, window, cx);
        let version_input = make("latest", None, window, cx);
        let timeout = make("5m0s", Some(settings.timeout()), window, cx);
        let description = make("optional", None, window, cx);
        let typed = typed_input(row.name.clone(), window, cx);
        let values = cx.new(|cx| ValuesEditor::new(String::new(), window, cx));
        let mut subscriptions: Vec<Subscription> = [&typed, &timeout, &description]
            .into_iter()
            .map(|input| {
                cx.subscribe(input, |this: &mut Self, _, event: &InputEvent, cx| {
                    if matches!(event, InputEvent::Change) {
                        this.error = None;
                        cx.notify();
                    }
                })
            })
            .collect();
        subscriptions.push(cx.subscribe_in(
            &chart_input,
            window,
            |this, _, event: &InputEvent, window, cx| {
                if let InputEvent::PressEnter { .. } = event {
                    let text = this.chart_input.read(cx).value().to_string();
                    match repo::parse_reference(&text) {
                        Some((chart, version)) => {
                            if let Some(version) = version {
                                this.version = Some(version.clone());
                                this.version_input
                                    .update(cx, |input, cx| input.set_value(version, window, cx));
                            }
                            this.set_chart(chart, window, cx);
                        }
                        None => {
                            this.error = Some(
                                "Type repo/chart or oci://registry/path/chart:version.".into(),
                            );
                            cx.notify();
                        }
                    }
                }
            },
        ));
        subscriptions.push(cx.subscribe_in(
            &version_input,
            window,
            |this, _, event: &InputEvent, window, cx| {
                if let InputEvent::PressEnter { .. } = event {
                    let version = this.version_input.read(cx).value().trim().to_string();
                    this.version = (!version.is_empty()).then_some(version);
                    this.load_details(window, cx);
                }
            },
        ));
        subscriptions.push(cx.observe(&values, |_, _, cx| cx.notify()));
        if let Some(ops) = HelmOps::global(cx) {
            subscriptions.push(cx.observe_in(&ops, window, |this, _, window, cx| {
                this.check_done(window, cx)
            }));
        }
        let mut this = Self {
            cluster,
            row,
            helm,
            current: None,
            candidates: Vec::new(),
            chart: None,
            chart_input,
            versions: Vec::new(),
            version: None,
            version_input,
            timeout,
            description,
            typed,
            wait: false,
            atomic: settings.atomic,
            values_mode: Values::Edit,
            values,
            step: Step::Failed(String::new()),
            error: None,
            focus: cx.focus_handle(),
            _versions_task: None,
            _details_task: None,
            _subscriptions: subscriptions,
        };
        this.load(window, cx);
        this
    }

    /// Reads the current release and looks for its chart in the added repositories.
    fn load(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Some(client) = client(&self.cluster, cx) else {
            self.step = Step::Failed("The cluster isn't connected.".into());
            return;
        };
        let (driver, namespace, object) = (
            self.row.driver,
            self.row.namespace.clone(),
            self.row.latest().object.clone(),
        );
        let helm = self.helm.clone();
        let work = spawn_kube(cx, async move {
            let release = crate::service::load(client, driver, namespace, object).await?;
            let chart = release.summary.chart.name.clone();
            let candidates =
                match cli::run(&helm, None, repo::search(&chart, false), None, None).await {
                    Ok(output) => repo::parse_search(&output.stdout)
                        .unwrap_or_default()
                        .into_iter()
                        .filter(|hit| hit.chart.name() == chart)
                        .map(|hit| hit.chart)
                        .collect(),
                    Err(_) => Vec::new(),
                };
            Ok::<_, String>((release, candidates))
        });
        let task = cx.spawn_in(window, async move |this, cx| {
            let result = work.await;
            this.update_in(cx, |this, window, cx| {
                match result {
                    Ok((release, candidates)) => {
                        let text = preview::values_yaml(&release.values);
                        this.values
                            .update(cx, |editor, cx| editor.set_text(text, window, cx));
                        // Stay on the installed version until another one is picked: a values
                        // change doesn't move the release to a newer chart.
                        let installed = release.summary.chart.version.clone();
                        if !installed.is_empty() {
                            this.version = Some(installed.clone());
                            this.version_input
                                .update(cx, |input, cx| input.set_value(installed, window, cx));
                        }
                        this.current = Some(Arc::new(release));
                        this.candidates = candidates;
                        this.step = Step::Configure;
                        // One repository has the chart: that's it. Several: the user picks (the
                        // release doesn't record where its chart came from).
                        if let [chart] = this.candidates.as_slice() {
                            let chart = chart.clone();
                            this.set_chart(chart, window, cx);
                        }
                    }
                    Err(err) => this.step = Step::Failed(err),
                }
                cx.notify();
            })
            .ok();
        });
        self.step = Step::Loading(task);
    }

    fn set_chart(&mut self, chart: ChartRef, window: &mut Window, cx: &mut Context<Self>) {
        self.chart = Some(chart.clone());
        self.versions.clear();
        self._details_task = None;
        // The last chart's schema doesn't check this one's values.
        self.values
            .update(cx, |editor, cx| editor.set_schema(None, true, cx));
        if let ChartRef::Repo {
            repo: repo_name,
            name,
        } = chart
        {
            let helm = self.helm.clone();
            let work = spawn_kube(cx, async move {
                let output = cli::run(&helm, None, repo::versions(&repo_name, &name), None, None)
                    .await
                    .map_err(|e| e.message)?;
                repo::parse_versions(&output.stdout, &repo_name, &name)
            });
            self._versions_task = Some(cx.spawn_in(window, async move |this, cx| {
                let result = work.await;
                this.update_in(cx, |this, window, cx| {
                    if let Ok(hits) = result {
                        this.versions = hits
                            .into_iter()
                            .map(|h| (h.version, h.app_version))
                            .collect();
                    }
                    this.load_details(window, cx);
                    cx.notify();
                })
                .ok();
            }));
        } else {
            self._versions_task = None;
            self.load_details(window, cx);
        }
        cx.notify();
    }

    fn chosen_version(&self, cx: &App) -> Option<String> {
        if self.versions.is_empty() {
            let typed = self.version_input.read(cx).value().trim().to_string();
            (!typed.is_empty())
                .then_some(typed)
                .or(self.version.clone())
        } else {
            self.version.clone()
        }
    }

    /// The new version's schema (the values are checked as overrides).
    fn load_details(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Some(chart) = self.chart.clone() else {
            return;
        };
        let version = self.chosen_version(cx);
        let helm = self.helm.clone();
        let work = spawn_kube(cx, async move {
            repo::details(&helm, &chart, version.as_deref()).await
        });
        self._details_task = Some(cx.spawn_in(window, async move |this, cx| {
            let result = work.await;
            this.update_in(cx, |this, _, cx| {
                // No details (a failed pull): no schema, rather than another version's.
                let schema = result.ok().and_then(|details| details.schema);
                this.values
                    .update(cx, |editor, cx| editor.set_schema(schema, true, cx));
                cx.notify();
            })
            .ok();
        }));
    }

    fn set_values_mode(&mut self, mode: Values, window: &mut Window, cx: &mut Context<Self>) {
        if self.values_mode == mode {
            return;
        }
        self.values_mode = mode;
        let text = match (mode, &self.current) {
            (Values::Edit, Some(current)) => preview::values_yaml(&current.values),
            _ => String::new(),
        };
        self.values
            .update(cx, |editor, cx| editor.set_text(text, window, cx));
        cx.notify();
    }

    fn spec(&self, cx: &App) -> Result<UpgradeSpec, String> {
        let chart = self.chart.clone().ok_or(if self.candidates.len() > 1 {
            "Several repositories have this chart: pick the one the release comes from."
        } else {
            "Kubyl can't tell which repository the chart comes from: type repo/chart or an oci:// reference and press Enter."
        })?;
        chart.validate()?;
        let timeout = self.timeout.read(cx).value().trim().to_string();
        if !timeout.is_empty() && cmd::parse_duration(&timeout).is_none() {
            return Err("The timeout is a duration like 5m0s or 90s.".into());
        }
        let description = self.description.read(cx).value().trim().to_string();
        Ok(UpgradeSpec {
            name: self.row.name.clone(),
            namespace: self.row.namespace.clone(),
            driver: self.row.driver,
            chart,
            version: self.chosen_version(cx),
            values_mode: match self.values_mode {
                Values::Reuse => ValuesMode::Reuse,
                Values::Edit | Values::Reset => ValuesMode::Replace,
            },
            options: WriteOptions {
                wait: self.wait,
                timeout,
                atomic: self.atomic,
                skip_crds: false,
                description: (!description.is_empty()).then_some(description),
            },
        })
    }

    fn blocker(&self, cx: &App) -> Option<String> {
        if let Err(err) = self.spec(cx) {
            return Some(err);
        }
        let editor = self.values.read(cx);
        if editor.errors() > 0 {
            return Some("Fix the values first (see the problems under the editor).".into());
        }
        editor.values(cx).err()
    }

    fn preview(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if let Some(blocker) = self.blocker(cx) {
            self.error = Some(blocker);
            cx.notify();
            return;
        }
        let (Ok(spec), Some(current)) = (self.spec(cx), self.current.clone()) else {
            return;
        };
        let values = self.values.read(cx).text(cx);
        let helm = self.helm.clone();
        let target = crate::cli::target(&self.cluster, cx);
        let work = spawn_kube(cx, async move {
            let preview =
                preview::upgrade_dry_run(&helm, target.as_ref(), &spec, &values, &current).await?;
            Ok::<_, String>((preview, spec, values))
        });
        let task = cx.spawn_in(window, async move |this, cx| {
            let result = work.await;
            this.update_in(cx, |this, window, cx| {
                match result {
                    Ok((preview, mut spec, values)) => {
                        spec.version = preview::rendered_version(&preview).or(spec.version);
                        let pane =
                            cx.new(|_| PreviewPane::new(PreviewKind::Upgrade, Arc::new(preview)));
                        this.step = Step::Preview {
                            pane,
                            spec: Box::new(spec),
                            values,
                        };
                        // A confirmation is typed for this review, not an earlier one.
                        this.typed
                            .update(cx, |input, cx| input.set_value("", window, cx));
                    }
                    Err(err) => {
                        this.error = Some(err);
                        this.step = Step::Configure;
                    }
                }
                cx.notify();
            })
            .ok();
        });
        self.error = None;
        self.step = Step::Previewing(task);
        cx.notify();
    }

    fn typed_ok(&self, cx: &App) -> bool {
        !production(&self.cluster, cx) || self.typed.read(cx).value().trim() == self.row.name
    }

    fn upgrade(&mut self, cx: &mut Context<Self>) {
        if !self.typed_ok(cx) {
            self.error =
                Some("Type the release's name to upgrade it on a production cluster.".into());
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
        // Exactly what the review showed.
        let Step::Preview { spec, values, .. } = &self.step else {
            return;
        };
        let (spec, values) = (spec.as_ref().clone(), values.clone());
        let start = Start {
            cluster: self.cluster.clone(),
            kind: OpKind::Upgrade,
            namespace: spec.namespace.clone(),
            release: spec.name.clone(),
            helm: self.helm.clone(),
            target: crate::cli::target(&self.cluster, cx),
            invocation: cmd::upgrade(&spec, &values, Mode::Apply, self.helm.version),
        };
        if let Some(id) = crate::ops::start(start, cx) {
            self.step = Step::Running(id);
        }
        cx.notify();
    }

    fn check_done(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Step::Running(id) = self.step else {
            return;
        };
        let state =
            HelmOps::global(cx).and_then(|ops| ops.read(cx).get(id).map(|op| op.state.clone()));
        if let Some(OpState::Done { revision }) = state {
            let revision = revision.unwrap_or(self.row.latest().revision + 1);
            let object = format!("sh.helm.release.v1.{}.v{revision}", self.row.name);
            let target = crate::service::object_ref(
                &self.cluster,
                self.row.driver,
                &self.row.namespace,
                &object,
            );
            window.close_dialog(cx);
            window.dispatch_action(
                Box::new(OpenView(ViewRequest::for_resource(
                    ViewKind::Custom(crate::release::VIEW_KIND.into()),
                    target,
                ))),
                cx,
            );
        }
        cx.notify();
    }

    fn render_chart(&self, cx: &mut Context<Self>) -> AnyElement {
        let colors = cx.colors().clone();
        if self.candidates.len() > 1 {
            let current = self.chart.clone();
            let candidates = self.candidates.clone();
            let weak = cx.entity().downgrade();
            return MenuButton::new("upgrade-chart")
                .outline()
                .w_full()
                .child(
                    h_flex()
                        .w_full()
                        .gap(u(6.0))
                        .child(div().font_family(fonts::MONO).text_size(u(12.0)).child(
                            current.as_ref().map(|c| c.label()).unwrap_or_else(|| {
                                "Pick the repository the release comes from…".into()
                            }),
                        ))
                        .child(div().flex_1())
                        .child(Icon::new(IconName::ChevronDown).size(11.0)),
                )
                .dropdown_menu(move |mut menu, _, _| {
                    for chart in &candidates {
                        let weak = weak.clone();
                        let pick = chart.clone();
                        menu = menu.item(
                            PopupMenuItem::new(chart.label())
                                .checked(current.as_ref() == Some(chart))
                                .on_click(move |_, window, cx| {
                                    let pick = pick.clone();
                                    weak.update(cx, |this, cx| this.set_chart(pick, window, cx))
                                        .ok();
                                }),
                        );
                    }
                    menu
                })
                .into_any_element();
        }
        match &self.chart {
            Some(chart) => div()
                .font_family(fonts::MONO)
                .text_size(u(12.5))
                .child(chart.label())
                .into_any_element(),
            None => v_flex()
                .gap(u(4.0))
                .child(input_box(&self.chart_input, true, &colors))
                .child(widgets::muted(
                    "The release doesn't record its repository, and no added repository has this chart. Type a reference and press Enter.",
                    &colors,
                ))
                .into_any_element(),
        }
    }

    fn render_version(&self, cx: &mut Context<Self>) -> AnyElement {
        let colors = cx.colors().clone();
        if self.versions.is_empty() {
            return input_box(&self.version_input, true, &colors).into_any_element();
        }
        let current = self.version.clone();
        let installed = self
            .current
            .as_ref()
            .map(|r| r.summary.chart.version.clone())
            .unwrap_or_default();
        let label_of = |version: &str| {
            let mut label = super::version_label(&self.versions, version);
            if version == installed {
                label.push_str(" · installed");
            }
            label
        };
        let label = current.as_deref().map(label_of).unwrap_or_default();
        let items: Vec<(String, String)> = self
            .versions
            .iter()
            .map(|(version, app)| {
                let mut label = super::version_label(&self.versions, version);
                if let Some(app) = app {
                    label.push_str(&format!(" · app {app}"));
                }
                if *version == installed {
                    label.push_str(" · installed");
                }
                (version.clone(), label)
            })
            .collect();
        let weak = cx.entity().downgrade();
        MenuButton::new("upgrade-version")
            .outline()
            .w_full()
            .child(
                h_flex()
                    .w_full()
                    .gap(u(6.0))
                    .child(
                        div()
                            .font_family(fonts::MONO)
                            .text_size(u(12.0))
                            .child(label),
                    )
                    .child(div().flex_1())
                    .child(Icon::new(IconName::ChevronDown).size(11.0)),
            )
            .dropdown_menu(move |mut menu, _, _| {
                menu = menu.max_h(px(320.0)).scrollable(true);
                for (version, label) in &items {
                    let weak = weak.clone();
                    let pick = version.clone();
                    menu = menu.item(
                        PopupMenuItem::new(label.clone())
                            .checked(current.as_deref() == Some(version.as_str()))
                            .on_click(move |_, window, cx| {
                                let pick = pick.clone();
                                weak.update(cx, |this, cx| {
                                    this.version = Some(pick);
                                    this.load_details(window, cx);
                                    cx.notify();
                                })
                                .ok();
                            }),
                    );
                }
                menu
            })
            .into_any_element()
    }

    fn render_configure(&self, cx: &mut Context<Self>) -> AnyElement {
        let colors = cx.colors().clone();
        let installed = self
            .current
            .as_ref()
            .map(|r| {
                format!(
                    "{} {}{}",
                    r.summary.chart.name,
                    r.summary.chart.version,
                    r.summary
                        .chart
                        .app_version
                        .as_ref()
                        .map(|a| format!(" · app {a}"))
                        .unwrap_or_default()
                )
            })
            .unwrap_or_default();
        let atomic = if self.helm.version.is_v4() {
            "Roll back on failure (--rollback-on-failure)"
        } else {
            "Roll back on failure (--atomic)"
        };
        let left = v_flex()
            .flex_none()
            .w(u(380.0))
            .h_full()
            .p(u(16.0))
            .gap(u(12.0))
            .border_r_1()
            .border_color(colors.border_variant)
            .child(form_row(
                "Installed",
                div()
                    .font_family(fonts::MONO)
                    .text_size(u(12.5))
                    .text_color(colors.text_muted)
                    .child(installed),
                &colors,
            ))
            .child(form_row("Chart", self.render_chart(cx), &colors))
            .child(form_row("Version", self.render_version(cx), &colors))
            .child(form_row(
                "Description",
                input_box(&self.description, false, &colors),
                &colors,
            ))
            .child(
                v_flex()
                    .gap(u(8.0))
                    .child(widgets::dialog_title("Options", &colors))
                    .child(
                        checkbox("upgrade-atomic", self.atomic, atomic, &colors).on_click(
                            cx.listener(|this, _, _, cx| {
                                this.atomic = !this.atomic;
                                cx.notify();
                            }),
                        ),
                    )
                    .child(
                        checkbox(
                            "upgrade-wait",
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
                        h_flex()
                            .gap(u(8.0))
                            .text_size(u(12.5))
                            .child("Timeout")
                            .child(
                                div()
                                    .w(u(90.0))
                                    .child(input_box(&self.timeout, true, &colors)),
                            ),
                    ),
            );
        let mode_note = match self.values_mode {
            Values::Edit => {
                "The release's user-supplied values: what's here replaces them (--reset-values), the new chart's defaults fill the rest."
            }
            Values::Reuse => {
                "The last release's values are reused (--reuse-values); values here are merged on top."
            }
            Values::Reset => {
                "Back to the chart's defaults (--reset-values); values here are applied on top."
            }
        };
        let right = v_flex()
            .flex_1()
            .min_w_0()
            .h_full()
            .p(u(16.0))
            .gap(u(8.0))
            .child(
                h_flex()
                    .gap(u(6.0))
                    .child(widgets::dialog_title("Values", &colors))
                    .child(div().w(u(8.0)))
                    .child(widgets::toggle_chip(
                        "upgrade-values-edit",
                        "Edit current values",
                        self.values_mode == Values::Edit,
                        cx.listener(|this, _, window, cx| {
                            this.set_values_mode(Values::Edit, window, cx)
                        }),
                    ))
                    .child(widgets::toggle_chip(
                        "upgrade-values-reuse",
                        "Reuse values",
                        self.values_mode == Values::Reuse,
                        cx.listener(|this, _, window, cx| {
                            this.set_values_mode(Values::Reuse, window, cx)
                        }),
                    ))
                    .child(widgets::toggle_chip(
                        "upgrade-values-reset",
                        "Reset to chart defaults",
                        self.values_mode == Values::Reset,
                        cx.listener(|this, _, window, cx| {
                            this.set_values_mode(Values::Reset, window, cx)
                        }),
                    )),
            )
            .child(div().flex_1().min_h_0().child(self.values.clone()))
            .child(widgets::muted(
                format!("{mode_note} Values go to helm on stdin, never into a file."),
                &colors,
            ));
        h_flex()
            .h(u(480.0))
            .items_start()
            .child(left)
            .child(right)
            .into_any_element()
    }
}

impl Focusable for UpgradeDialog {
    fn focus_handle(&self, _: &App) -> FocusHandle {
        self.focus.clone()
    }
}

impl Render for UpgradeDialog {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let colors = cx.colors().clone();
        let prod = production(&self.cluster, cx);
        let step_index = match &self.step {
            Step::Preview { .. } => 1,
            Step::Running(_) => 2,
            _ => 0,
        };
        let mut extra = vec![super::steps(
            &["Configure", "Review", "Upgrade"],
            step_index,
            &colors,
        )];
        if prod {
            extra.push(ProdBadge.into_any_element());
        }
        let lead = Icon::new(IconName::ArrowUp)
            .size(16.0)
            .color(colors.accent)
            .into_any_element();
        let title = format!("Upgrade {} in {}", self.row.name, self.row.namespace);
        let body: AnyElement = match &self.step {
            Step::Loading(_) => v_flex()
                .h(u(480.0))
                .items_center()
                .justify_center()
                .child(widgets::muted("Reading the release…", &colors))
                .into_any_element(),
            Step::Failed(err) => v_flex()
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
            Step::Configure => self.render_configure(cx),
            Step::Previewing(_) => v_flex()
                .h(u(480.0))
                .items_center()
                .justify_center()
                .gap(u(8.0))
                .child(
                    Icon::new(IconName::RefreshCw)
                        .size(18.0)
                        .color(colors.accent),
                )
                .child(widgets::muted(
                    "Rendering the upgrade with a server-side dry run (nothing is applied)…",
                    &colors,
                ))
                .into_any_element(),
            Step::Preview { pane, .. } => div().h(u(480.0)).child(pane.clone()).into_any_element(),
            Step::Running(id) => super::operation(*id, cx),
        };
        let summary = match (&self.current, &self.chart) {
            (Some(current), Some(chart)) => format!(
                "{} {} → {} · {}",
                chart.label(),
                current.summary.chart.version,
                match &self.step {
                    Step::Preview { spec, .. } => spec.version.clone(),
                    _ => self.chosen_version(cx),
                }
                .unwrap_or_else(|| "latest".into()),
                match self.values_mode {
                    Values::Edit => "values: edited (--reset-values)",
                    Values::Reuse => "values: reused (--reuse-values)",
                    Values::Reset => "values: chart defaults (--reset-values)",
                }
            ),
            _ => String::new(),
        };
        let note =
            error_line(&self.error, &colors).unwrap_or_else(|| widgets::muted(summary, &colors));
        let buttons: Vec<AnyElement> = match &self.step {
            Step::Loading(_) | Step::Failed(_) => vec![super::close_button("Cancel")],
            Step::Configure | Step::Previewing(_) => vec![
                super::close_button("Cancel"),
                Button::new("upgrade-preview")
                    .primary()
                    .icon(IconName::Eye)
                    .label("Review")
                    .disabled(
                        matches!(self.step, Step::Previewing(_)) || self.blocker(cx).is_some(),
                    )
                    .on_click(cx.listener(|this, _, window, cx| this.preview(window, cx)))
                    .into_any_element(),
            ],
            Step::Preview { .. } => {
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
                buttons.push(
                    Button::new("upgrade-back")
                        .ghost()
                        .icon(IconName::ArrowLeft)
                        .label("Back")
                        .on_click(cx.listener(|this, _, _, cx| {
                            this.step = Step::Configure;
                            cx.notify();
                        }))
                        .into_any_element(),
                );
                buttons.push(
                    Button::new("upgrade-run")
                        .primary()
                        .icon(IconName::ArrowUp)
                        .label("Upgrade")
                        .disabled(!self.typed_ok(cx))
                        .on_click(cx.listener(|this, _, _, cx| this.upgrade(cx)))
                        .into_any_element(),
                );
                buttons
            }
            Step::Running(id) => {
                let done = HelmOps::global(cx)
                    .and_then(|ops| ops.read(cx).get(*id).map(|op| !op.is_running()))
                    .unwrap_or(true);
                vec![super::close_button(if done { "Close" } else { "Hide" })]
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
