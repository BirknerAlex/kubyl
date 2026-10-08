//! Install a chart (board 20): release name, namespace (existing or created), version, values
//! (the chart's defaults to edit, or overrides only; checked against `values.schema.json`) and
//! options; then the server-side dry run's preview; then `helm install`, with Helm's output in
//! the dialog. On success the new release opens in its tab.

use std::sync::Arc;

use gpui::{
    AnyElement, App, AppContext as _, Context, Entity, FocusHandle, Focusable, IntoElement, Render,
    Subscription, Task, Window, div, prelude::*, px,
};
use gpui_component::WindowExt as _;
use gpui_component::button::{Button as MenuButton, ButtonVariants as _};
use gpui_component::input::{InputEvent, InputState};
use gpui_component::menu::{DropdownMenu as _, PopupMenuItem};
use kubyl_core::actions::OpenView;
use kubyl_core::{ClusterId, ViewKind, ViewRequest, spawn_kube};
use kubyl_helm_core::cli::{self, HelmInfo};
use kubyl_helm_core::cmd::{self, InstallSpec, Mode, WriteOptions};
use kubyl_helm_core::decode::Driver;
use kubyl_helm_core::preview;
use kubyl_helm_core::repo::{self, ChartDetails, ChartRef};
use kubyl_kube::ConnectionManager;
use kubyl_ui::{ActiveColors, Button, Icon, IconName, ProdBadge, fonts, h_flex, u, v_flex};

use super::preview::{PreviewKind, PreviewPane};
use super::{
    checkbox, cluster_name, error_line, footer, form_row, guard, header, input_box, open,
    production, typed_input,
};
use crate::cli::HelmCli;
use crate::ops::{HelmOps, OpKind, OpState, Start};
use crate::values_editor::ValuesEditor;
use crate::widgets;

/// What to install.
#[derive(Clone, Debug, Default)]
pub struct InstallRequest {
    pub cluster: Option<ClusterId>,
    /// `None`: the user types a reference (`repo/chart`, `oci://…`).
    pub chart: Option<ChartRef>,
    pub version: Option<String>,
    pub namespace: Option<String>,
}

/// Opens the install dialog (or explains how to get `helm`).
pub fn open_install(request: InstallRequest, window: &mut Window, cx: &mut App) {
    let Some(cluster) = request.cluster.clone().or_else(|| {
        kubyl_core::ActiveContext::global(cx)
            .cluster
            .as_ref()
            .map(|c| c.id.clone())
    }) else {
        kubyl_core::NotificationCenter::push(
            cx,
            kubyl_core::Notification::error("Pick a cluster first."),
        );
        return;
    };
    if !guard(&cluster, cx) {
        return;
    }
    let Some(helm) = HelmCli::info(cx) else {
        super::open_missing(window, cx);
        return;
    };
    let view = cx.new(|cx| InstallDialog::new(cluster, request, helm, window, cx));
    let focus = view.read(cx).name.read(cx).focus_handle(cx);
    open(view, 1000.0, Some(focus), window, cx);
}

/// Values start from the chart's defaults, or hold only overrides.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum ValuesMode {
    Defaults,
    Overrides,
}

enum Details {
    None,
    Loading(#[allow(dead_code)] Task<()>),
    Ready(Arc<ChartDetails>),
    Failed(String),
}

enum Step {
    Configure,
    Previewing(#[allow(dead_code)] Task<()>),
    /// The dry run's preview, and exactly what it ran: Install applies that (with the chart
    /// version the dry run rendered), never a command rebuilt from the form.
    Preview {
        pane: Entity<PreviewPane>,
        spec: Box<InstallSpec>,
        values: String,
    },
    Running(u64),
}

pub struct InstallDialog {
    cluster: ClusterId,
    helm: HelmInfo,
    chart: Option<ChartRef>,
    chart_input: Entity<InputState>,
    versions: Vec<(String, Option<String>)>,
    version: Option<String>,
    version_input: Entity<InputState>,
    details: Details,
    details_key: Option<(ChartRef, Option<String>)>,
    pub(crate) name: Entity<InputState>,
    namespace: Entity<InputState>,
    description: Entity<InputState>,
    timeout: Entity<InputState>,
    typed: Entity<InputState>,
    create_namespace: bool,
    wait: bool,
    atomic: bool,
    skip_crds: bool,
    mode: ValuesMode,
    values: Entity<ValuesEditor>,
    step: Step,
    error: Option<String>,
    focus: FocusHandle,
    _versions_task: Option<Task<()>>,
    _subscriptions: Vec<Subscription>,
}

impl InstallDialog {
    fn new(
        cluster: ClusterId,
        request: InstallRequest,
        helm: HelmInfo,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Self {
        let settings = crate::cli::settings(cx);
        let input = |placeholder: &str,
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
        let default_name = request.chart.as_ref().map(|c| c.name().to_string());
        let namespace = request.namespace.clone().or_else(|| {
            let active = kubyl_core::ActiveContext::global(cx);
            (active.cluster.as_ref().map(|c| &c.id) == Some(&cluster))
                .then(|| active.namespace.as_ref().map(|n| n.to_string()))
                .flatten()
        });
        let chart_input = input(
            "repo/chart, or oci://registry/path/chart:version",
            None,
            window,
            cx,
        );
        let version_input = input("latest", request.version.clone(), window, cx);
        let name = input("release name", default_name, window, cx);
        let namespace = input(
            "namespace",
            Some(namespace.unwrap_or_else(|| "default".into())),
            window,
            cx,
        );
        let description = input("optional", None, window, cx);
        let timeout = input("5m0s", Some(settings.timeout()), window, cx);
        let typed = typed_input(cluster_name(&cluster, cx), window, cx);
        let values = cx.new(|cx| ValuesEditor::new(String::new(), window, cx));
        let mut subscriptions: Vec<Subscription> = [&name, &typed, &description, &timeout]
            .into_iter()
            .map(|input| {
                cx.subscribe_in(input, window, |this, _, event: &InputEvent, _, cx| {
                    if matches!(event, InputEvent::Change) {
                        this.error = None;
                        cx.notify();
                    }
                })
            })
            .collect();
        // Only the namespace decides `--create-namespace` (typing elsewhere keeps the checkbox).
        subscriptions.push(cx.subscribe_in(
            &namespace,
            window,
            |this, _, event: &InputEvent, _, cx| {
                if matches!(event, InputEvent::Change) {
                    this.error = None;
                    this.sync_namespace(cx);
                    cx.notify();
                }
            },
        ));
        subscriptions.push(cx.subscribe_in(
            &chart_input,
            window,
            |this, _, event: &InputEvent, window, cx| match event {
                InputEvent::Change => {
                    this.error = None;
                    cx.notify();
                }
                InputEvent::PressEnter { .. } => this.use_typed_chart(window, cx),
                _ => {}
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
            helm,
            chart: request.chart.clone(),
            chart_input,
            versions: Vec::new(),
            version: request.version.clone(),
            version_input,
            details: Details::None,
            details_key: None,
            name,
            namespace,
            description,
            timeout,
            typed,
            create_namespace: false,
            wait: false,
            atomic: settings.atomic,
            skip_crds: false,
            mode: ValuesMode::Defaults,
            values,
            step: Step::Configure,
            error: None,
            focus: cx.focus_handle(),
            _versions_task: None,
            _subscriptions: subscriptions,
        };
        this.sync_namespace(cx);
        if this.chart.is_some() {
            this.load_versions(window, cx);
            this.load_details(window, cx);
        }
        this
    }

    /// `--create-namespace` follows whether the namespace exists.
    fn sync_namespace(&mut self, cx: &mut Context<Self>) {
        let namespace = self.namespace.read(cx).value().trim().to_string();
        let namespaces = ConnectionManager::try_global(cx)
            .map(|m| m.read(cx).namespaces(&self.cluster))
            .unwrap_or_default();
        // Only a live list proves the namespace is missing: the fallback list (kubeconfig,
        // settings) lacks namespaces a restricted user owns, and creating one would be forbidden.
        self.create_namespace = namespaces.listed && !namespaces.names.contains(&namespace);
    }

    fn use_typed_chart(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let text = self.chart_input.read(cx).value().to_string();
        match repo::parse_reference(&text) {
            Some((chart, version)) => {
                if self.name.read(cx).value().trim().is_empty() {
                    let name = chart.name().to_string();
                    self.name
                        .update(cx, |input, cx| input.set_value(name, window, cx));
                }
                self.chart = Some(chart);
                self.version = version.clone();
                if let Some(version) = version {
                    self.version_input
                        .update(cx, |input, cx| input.set_value(version, window, cx));
                }
                self.load_versions(window, cx);
                self.load_details(window, cx);
            }
            None => {
                self.error = Some(
                    "Type repo/chart (an added repository) or oci://registry/path/chart:version."
                        .into(),
                )
            }
        }
        cx.notify();
    }

    /// The versions of a repository chart (OCI references have no index: the user types one).
    fn load_versions(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Some(ChartRef::Repo {
            repo: name_repo,
            name,
        }) = self.chart.clone()
        else {
            return;
        };
        let helm = self.helm.clone();
        let work = spawn_kube(cx, async move {
            let output = cli::run(&helm, None, repo::versions(&name_repo, &name), None, None)
                .await
                .map_err(|e| e.message)?;
            repo::parse_versions(&output.stdout, &name_repo, &name)
        });
        self._versions_task = Some(cx.spawn_in(window, async move |this, cx| {
            let result = work.await;
            this.update_in(cx, |this, window, cx| {
                if let Ok(hits) = result {
                    this.versions = hits
                        .into_iter()
                        .map(|h| (h.version, h.app_version))
                        .collect();
                    this.pick_default_version(window, cx);
                }
                cx.notify();
            })
            .ok();
        }));
    }

    /// Without a version asked for, the one `helm install` would pick (the newest that isn't a
    /// pre-release), named explicitly from now on. The details pulled without a version are
    /// that one's.
    fn pick_default_version(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.version.is_some() || self.versions.is_empty() {
            return;
        }
        let stable =
            repo::latest_stable(self.versions.iter().map(|(v, _)| v.as_str())).map(str::to_string);
        if let (Some(stable), Some((chart, None))) = (&stable, &self.details_key) {
            self.details_key = Some((chart.clone(), Some(stable.clone())));
        }
        self.version = stable.or_else(|| self.versions.first().map(|(v, _)| v.clone()));
        self.load_details(window, cx);
    }

    fn version_label(&self, version: &str) -> String {
        super::version_label(&self.versions, version)
    }

    fn chosen_version(&self, cx: &App) -> Option<String> {
        if self.versions.is_empty() {
            let typed = self.version_input.read(cx).value().trim().to_string();
            (!typed.is_empty()).then_some(typed)
        } else {
            self.version.clone()
        }
    }

    /// Pulls the chart for its default values, schema and CRDs.
    fn load_details(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Some(chart) = self.chart.clone() else {
            return;
        };
        let version = self.chosen_version(cx);
        let key = (chart.clone(), version.clone());
        if self.details_key.as_ref() == Some(&key) {
            return;
        }
        self.details_key = Some(key);
        let helm = self.helm.clone();
        let work = spawn_kube(cx, async move {
            repo::details(&helm, &chart, version.as_deref()).await
        });
        let task = cx.spawn_in(window, async move |this, cx| {
            let result = work.await;
            this.update_in(cx, |this, window, cx| {
                match result {
                    Ok(details) => {
                        let details = Arc::new(details);
                        this.apply_details(&details, window, cx);
                        this.details = Details::Ready(details);
                    }
                    Err(err) => {
                        // Enter on the same version tries again.
                        this.details_key = None;
                        this.details = Details::Failed(err);
                    }
                }
                cx.notify();
            })
            .ok();
        });
        self.details = Details::Loading(task);
        cx.notify();
    }

    fn apply_details(
        &mut self,
        details: &ChartDetails,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let partial = self.mode == ValuesMode::Overrides;
        let text = match self.mode {
            ValuesMode::Defaults => details.values.clone(),
            ValuesMode::Overrides => String::new(),
        };
        let schema = details.schema.clone();
        self.values.update(cx, |editor, cx| {
            // Keep what the user typed when only the version changed in override mode.
            if !partial || editor.text(cx).trim().is_empty() {
                editor.set_text(text, window, cx);
            }
            editor.set_schema(schema, partial, cx);
        });
    }

    fn set_mode(&mut self, mode: ValuesMode, window: &mut Window, cx: &mut Context<Self>) {
        if self.mode == mode {
            return;
        }
        self.mode = mode;
        if let Details::Ready(details) = &self.details {
            let details = details.clone();
            let text = match mode {
                ValuesMode::Defaults => details.values.clone(),
                ValuesMode::Overrides => String::new(),
            };
            self.values.update(cx, |editor, cx| {
                editor.set_text(text, window, cx);
                editor.set_schema(details.schema.clone(), mode == ValuesMode::Overrides, cx);
            });
        } else if mode == ValuesMode::Overrides {
            // Without this version's details the editor may hold another version's defaults.
            self.values
                .update(cx, |editor, cx| editor.set_text(String::new(), window, cx));
        }
        cx.notify();
    }

    /// The values to send (overrides only: an edited copy of the defaults becomes what changed).
    fn payload(&self, cx: &App) -> Result<String, String> {
        let text = self.values.read(cx).text(cx);
        let edited = kubyl_helm_core::values::parse(&text)?;
        match (&self.mode, &self.details) {
            (ValuesMode::Overrides, _) => Ok(text),
            (ValuesMode::Defaults, Details::Ready(details)) => {
                let defaults = kubyl_helm_core::values::parse(&details.values).unwrap_or_default();
                let overrides = kubyl_helm_core::values::overrides(&defaults, &edited);
                Ok(serde_json::to_string_pretty(&overrides).unwrap_or_default())
            }
            // The editor may still hold another version's defaults: sending them would pin
            // every one of them as the release's own values.
            (ValuesMode::Defaults, Details::Failed(_)) => {
                Err("The chart's default values couldn't be read: use override-only values.".into())
            }
            (ValuesMode::Defaults, Details::None | Details::Loading(_)) => {
                Err("Wait for the chart's default values.".into())
            }
        }
    }

    fn spec(&self, cx: &App) -> Result<InstallSpec, String> {
        let chart = self
            .chart
            .clone()
            .ok_or("Pick a chart: type repo/chart or an oci:// reference and press Enter.")?;
        chart.validate()?;
        let name = self.name.read(cx).value().trim().to_string();
        cmd::validate_release_name(&name)?;
        let namespace = self.namespace.read(cx).value().trim().to_string();
        cmd::validate_namespace(&namespace)?;
        let timeout = self.timeout.read(cx).value().trim().to_string();
        if !timeout.is_empty() && cmd::parse_duration(&timeout).is_none() {
            return Err("The timeout is a duration like 5m0s or 90s.".into());
        }
        let description = self.description.read(cx).value().trim().to_string();
        Ok(InstallSpec {
            name,
            namespace,
            create_namespace: self.create_namespace,
            chart,
            version: self.chosen_version(cx),
            options: WriteOptions {
                wait: self.wait,
                timeout,
                atomic: self.atomic,
                skip_crds: self.skip_crds,
                description: (!description.is_empty()).then_some(description),
            },
        })
    }

    /// Why Preview can't run yet (`None`: it can).
    pub(crate) fn blocker(&self, cx: &App) -> Option<String> {
        if let Err(err) = self.spec(cx) {
            return Some(err);
        }
        let editor = self.values.read(cx);
        if editor.errors() > 0 {
            return Some("Fix the values first (see the problems under the editor).".into());
        }
        if let Err(err) = editor.values(cx) {
            return Some(err);
        }
        self.payload(cx).err()
    }

    fn preview(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if let Some(blocker) = self.blocker(cx) {
            self.error = Some(blocker);
            cx.notify();
            return;
        }
        let (spec, values) = match (self.spec(cx), self.payload(cx)) {
            (Ok(spec), Ok(values)) => (spec, values),
            (Err(err), _) | (_, Err(err)) => {
                self.error = Some(err);
                cx.notify();
                return;
            }
        };
        let helm = self.helm.clone();
        let target = crate::cli::target(&self.cluster, cx);
        let work = spawn_kube(cx, async move {
            let preview = preview::install_dry_run(&helm, target.as_ref(), &spec, &values).await?;
            Ok::<_, String>((preview, spec, values))
        });
        let task = cx.spawn_in(window, async move |this, cx| {
            let result = work.await;
            this.update_in(cx, |this, window, cx| {
                match result {
                    Ok((preview, mut spec, values)) => {
                        spec.version = preview::rendered_version(&preview).or(spec.version);
                        let pane =
                            cx.new(|_| PreviewPane::new(PreviewKind::Install, Arc::new(preview)));
                        this.step = Step::Preview {
                            pane,
                            spec: Box::new(spec),
                            values,
                        };
                        // A confirmation is typed for this preview, not an earlier one.
                        this.typed
                            .update(cx, |input, cx| input.set_value("", window, cx));
                        let focus = this.typed.read(cx).focus_handle(cx);
                        if production(&this.cluster, cx) {
                            window.focus(&focus, cx);
                        }
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
        !production(&self.cluster, cx)
            || self.typed.read(cx).value().trim() == cluster_name(&self.cluster, cx)
    }

    fn install(&mut self, cx: &mut Context<Self>) {
        if !self.typed_ok(cx) {
            self.error = Some("Type the cluster's name to install on a production cluster.".into());
            cx.notify();
            return;
        }
        if !guard(&self.cluster, cx) {
            return;
        }
        // Exactly what the preview showed.
        let Step::Preview { spec, values, .. } = &self.step else {
            return;
        };
        let (spec, values) = (spec.as_ref().clone(), values.clone());
        let start = Start {
            cluster: self.cluster.clone(),
            kind: OpKind::Install,
            namespace: spec.namespace.clone(),
            release: spec.name.clone(),
            helm: self.helm.clone(),
            target: crate::cli::target(&self.cluster, cx),
            invocation: cmd::install(&spec, &values, Mode::Apply, self.helm.version),
        };
        if let Some(id) = crate::ops::start(start, cx) {
            self.step = Step::Running(id);
        }
        cx.notify();
    }

    /// The install finished: open the release's tab and close.
    fn check_done(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Step::Running(id) = self.step else {
            return;
        };
        let Some(op) = HelmOps::global(cx).and_then(|ops| {
            ops.read(cx)
                .get(id)
                .map(|op| (op.state.clone(), op.namespace.clone(), op.release.clone()))
        }) else {
            return;
        };
        if let (OpState::Done { revision }, namespace, release) = op {
            let object = format!("sh.helm.release.v1.{release}.v{}", revision.unwrap_or(1));
            let target =
                crate::service::object_ref(&self.cluster, Driver::Secret, &namespace, &object);
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

    fn render_chart_row(&self, cx: &mut Context<Self>) -> AnyElement {
        let colors = cx.colors().clone();
        match &self.chart {
            Some(chart) => h_flex()
                .gap(u(8.0))
                .child(
                    div()
                        .flex_1()
                        .min_w_0()
                        .truncate()
                        .font_family(fonts::MONO)
                        .text_size(u(12.5))
                        .child(chart.label()),
                )
                .into_any_element(),
            None => v_flex()
                .gap(u(4.0))
                .child(input_box(&self.chart_input, true, &colors))
                .child(widgets::muted("Press Enter to load the chart.", &colors))
                .into_any_element(),
        }
    }

    fn render_version_row(&self, cx: &mut Context<Self>) -> AnyElement {
        let colors = cx.colors().clone();
        if self.versions.is_empty() {
            return input_box(&self.version_input, true, &colors).into_any_element();
        }
        let current = self.version.clone();
        let label = current
            .as_deref()
            .map(|v| self.version_label(v))
            .unwrap_or_default();
        let app = self
            .versions
            .iter()
            .find(|(v, _)| Some(v) == current.as_ref())
            .and_then(|(_, a)| a.clone())
            .map(|a| format!("app {a}"))
            .unwrap_or_default();
        let items: Vec<(String, String)> = self
            .versions
            .iter()
            .map(|(version, app)| {
                let label = self.version_label(version);
                let label = match app {
                    Some(app) => format!("{label} · app {app}"),
                    None => label,
                };
                (version.clone(), label)
            })
            .collect();
        let weak = cx.entity().downgrade();
        MenuButton::new("install-version")
            .outline()
            .w_full()
            .child(
                h_flex()
                    .w_full()
                    .gap(u(8.0))
                    .child(
                        div()
                            .font_family(fonts::MONO)
                            .text_size(u(12.0))
                            .child(label),
                    )
                    .child(div().flex_1())
                    .child(
                        div()
                            .text_size(u(11.5))
                            .text_color(colors.text_dim)
                            .child(app),
                    )
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
                                })
                                .ok();
                            }),
                    );
                }
                menu
            })
            .into_any_element()
    }

    fn render_namespace_row(&self, cx: &mut Context<Self>) -> AnyElement {
        let colors = cx.colors().clone();
        let namespaces = ConnectionManager::try_global(cx)
            .map(|m| m.read(cx).namespaces(&self.cluster))
            .unwrap_or_default();
        // Not a live list: Kubyl can't tell which namespaces exist.
        let unknown = !namespaces.listed;
        let names = namespaces.names;
        let weak = cx.entity().downgrade();
        let menu = MenuButton::new("install-namespace-pick")
            .ghost()
            .compact()
            .child(Icon::new(IconName::ChevronDown).size(11.0))
            .dropdown_menu(move |mut menu, _, _| {
                menu = menu.max_h(px(320.0)).scrollable(true);
                for ns in &names {
                    let weak = weak.clone();
                    let ns = ns.clone();
                    menu = menu.item(PopupMenuItem::new(ns.clone()).on_click(
                        move |_, window, cx| {
                            let ns = ns.clone();
                            weak.update(cx, |this, cx| {
                                this.namespace
                                    .update(cx, |input, cx| input.set_value(ns, window, cx));
                                this.sync_namespace(cx);
                                cx.notify();
                            })
                            .ok();
                        },
                    ));
                }
                menu
            });
        v_flex()
            .gap(u(5.0))
            .child(
                h_flex()
                    .gap(u(4.0))
                    .child(
                        div()
                            .flex_1()
                            .child(input_box(&self.namespace, true, &colors)),
                    )
                    .child(menu),
            )
            .child(
                checkbox(
                    "install-create-namespace",
                    self.create_namespace,
                    "Create it (--create-namespace)",
                    &colors,
                )
                .on_click(cx.listener(|this, _, _, cx| {
                    this.create_namespace = !this.create_namespace;
                    cx.notify();
                })),
            )
            .when(unknown, |this| {
                this.child(widgets::muted(
                    "Kubyl can't list the namespaces here: check this when the namespace doesn't exist yet.",
                    &colors,
                ))
            })
            .into_any_element()
    }

    fn render_options(&self, cx: &mut Context<Self>) -> AnyElement {
        let colors = cx.colors().clone();
        let crds = match &self.details {
            Details::Ready(d) if !d.crds.is_empty() => {
                format!("Skip CRDs ({} in crds/)", d.crds.len())
            }
            _ => "Skip CRDs".to_string(),
        };
        let atomic = if self.helm.version.is_v4() {
            "Roll back on failure (--rollback-on-failure)"
        } else {
            "Roll back on failure (--atomic)"
        };
        v_flex()
            .gap(u(8.0))
            .child(widgets::dialog_title("Options", &colors))
            .child(
                checkbox("install-atomic", self.atomic, atomic, &colors).on_click(cx.listener(
                    |this, _, _, cx| {
                        this.atomic = !this.atomic;
                        cx.notify();
                    },
                )),
            )
            .child(
                checkbox(
                    "install-wait",
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
                checkbox("install-skip-crds", self.skip_crds, crds, &colors).on_click(cx.listener(
                    |this, _, _, cx| {
                        this.skip_crds = !this.skip_crds;
                        cx.notify();
                    },
                )),
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
            )
            .into_any_element()
    }

    fn render_values(&self, cx: &mut Context<Self>) -> AnyElement {
        let colors = cx.colors().clone();
        let schema = self.values.read(cx).has_schema();
        let state: AnyElement = match &self.details {
            Details::Loading(_) => widgets::muted("Pulling the chart…", &colors),
            Details::Failed(err) => widgets::note(
                IconName::TriangleAlert,
                colors.yellow,
                format!("Couldn't read the chart: {err}"),
                &colors,
            ),
            Details::Ready(_) if schema => h_flex()
                .gap(u(5.0))
                .text_size(u(12.0))
                .text_color(colors.text_dim)
                .child(
                    Icon::new(IconName::CircleCheck)
                        .size(12.0)
                        .color(colors.green),
                )
                .child("values.schema.json")
                .into_any_element(),
            Details::Ready(_) => widgets::muted("No values schema", &colors),
            Details::None => div().into_any_element(),
        };
        v_flex()
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
                        "install-values-defaults",
                        "Chart defaults",
                        self.mode == ValuesMode::Defaults,
                        cx.listener(|this, _, window, cx| {
                            this.set_mode(ValuesMode::Defaults, window, cx)
                        }),
                    ))
                    .child(widgets::toggle_chip(
                        "install-values-overrides",
                        "Override only",
                        self.mode == ValuesMode::Overrides,
                        cx.listener(|this, _, window, cx| {
                            this.set_mode(ValuesMode::Overrides, window, cx)
                        }),
                    ))
                    .child(div().flex_1())
                    .child(state),
            )
            .child(div().flex_1().min_h_0().child(self.values.clone()))
            .child(widgets::muted(
                match self.mode {
                    ValuesMode::Defaults => "Edit the chart's defaults: only what you change is sent (the release keeps your overrides, the chart its defaults). Values go to helm on stdin, never into a file.",
                    ValuesMode::Overrides => "Only these values are sent; the chart's defaults fill the rest. Values go to helm on stdin, never into a file.",
                },
                &colors,
            ))
            .into_any_element()
    }

    fn render_configure(&self, cx: &mut Context<Self>) -> AnyElement {
        let colors = cx.colors().clone();
        let left = v_flex()
            .flex_none()
            .w(u(380.0))
            .h_full()
            .p(u(16.0))
            .gap(u(12.0))
            .border_r_1()
            .border_color(colors.border_variant)
            .child(form_row("Chart", self.render_chart_row(cx), &colors))
            .child(form_row("Version", self.render_version_row(cx), &colors))
            .child(form_row(
                "Release name",
                input_box(&self.name, true, &colors),
                &colors,
            ))
            .child(form_row(
                "Namespace",
                self.render_namespace_row(cx),
                &colors,
            ))
            .child(form_row(
                "Description",
                input_box(&self.description, false, &colors),
                &colors,
            ))
            .child(self.render_options(cx));
        h_flex()
            .h(u(480.0))
            .items_start()
            .child(left)
            .child(self.render_values(cx))
            .into_any_element()
    }
}

#[cfg(test)]
impl InstallDialog {
    pub(crate) fn new_for_test(
        cluster: ClusterId,
        request: InstallRequest,
        helm: HelmInfo,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Self {
        Self::new(cluster, request, helm, window, cx)
    }

    /// Sets the chart without pulling it (tests run no `helm`).
    pub(crate) fn set_chart_for_test(&mut self, chart: ChartRef) {
        self.chart = Some(chart);
    }

    /// The chart's details as if pulled.
    pub(crate) fn set_details_for_test(&mut self, details: ChartDetails) {
        self.details = Details::Ready(Arc::new(details));
    }

    /// What Preview would send as values.
    pub(crate) fn payload_for_test(&self, cx: &App) -> Result<String, String> {
        self.payload(cx)
    }

    pub(crate) fn creates_namespace_for_test(&self) -> bool {
        self.create_namespace
    }

    pub(crate) fn set_field_for_test(
        &mut self,
        which: &str,
        text: &str,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let text = text.to_string();
        match which {
            "values" => self
                .values
                .update(cx, |editor, cx| editor.set_text(text, window, cx)),
            field => {
                let input = match field {
                    "name" => &self.name,
                    "namespace" => &self.namespace,
                    _ => &self.timeout,
                };
                input.update(cx, |input, cx| input.set_value(text, window, cx));
            }
        }
    }
}

impl Focusable for InstallDialog {
    fn focus_handle(&self, _: &App) -> FocusHandle {
        self.focus.clone()
    }
}

impl Render for InstallDialog {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let colors = cx.colors().clone();
        let prod = production(&self.cluster, cx);
        let chart_name = self
            .chart
            .as_ref()
            .map(|c| c.name().to_string())
            .unwrap_or_else(|| "a chart".into());
        let name = self.name.read(cx).value().trim().to_string();
        let (step_index, title) = match &self.step {
            Step::Configure | Step::Previewing(_) => (0, format!("Install {chart_name}")),
            Step::Preview { .. } => (1, format!("Install {chart_name} as {name}")),
            Step::Running(_) => (2, format!("Install {chart_name} as {name}")),
        };
        let lead = Icon::new(IconName::Anchor)
            .size(16.0)
            .color(colors.accent)
            .into_any_element();
        let mut extra = vec![super::steps(
            &["Configure", "Preview", "Install"],
            step_index,
            &colors,
        )];
        if prod {
            extra.push(ProdBadge.into_any_element());
        }
        let body: AnyElement = match &self.step {
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
                    "Rendering with a server-side dry run (nothing is applied)…",
                    &colors,
                ))
                .into_any_element(),
            Step::Preview { pane, .. } => div().h(u(480.0)).child(pane.clone()).into_any_element(),
            Step::Running(id) => div().child(super::operation(*id, cx)).into_any_element(),
        };
        let target = format!(
            "Into {} · helm {}",
            cluster_name(&self.cluster, cx),
            self.helm.version
        );
        let note =
            error_line(&self.error, &colors).unwrap_or_else(|| widgets::muted(target, &colors));
        let buttons: Vec<AnyElement> = match &self.step {
            Step::Configure | Step::Previewing(_) => vec![
                super::close_button("Cancel"),
                Button::new("install-preview")
                    .primary()
                    .icon(IconName::Eye)
                    .label("Preview")
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
                            .child(format!("Type {}", cluster_name(&self.cluster, cx)))
                            .child(
                                div()
                                    .w(u(180.0))
                                    .child(input_box(&self.typed, true, &colors)),
                            )
                            .into_any_element(),
                    );
                }
                buttons.push(
                    Button::new("install-back")
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
                    Button::new("install-run")
                        .primary()
                        .icon(IconName::Download)
                        .label("Install")
                        .disabled(!self.typed_ok(cx))
                        .on_click(cx.listener(|this, _, _, cx| this.install(cx)))
                        .into_any_element(),
                );
                buttons
            }
            Step::Running(id) => {
                let failed = HelmOps::global(cx)
                    .and_then(|ops| ops.read(cx).get(*id).map(|op| !op.is_running()))
                    .unwrap_or(true);
                vec![super::close_button(if failed { "Close" } else { "Hide" })]
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
