//! Dialogs (board 7): install an operator, review and approve an upgrade, uninstall, pick a kind
//! to create, copy a `helm` command.
//!
//! Every write shows what it creates or deletes first, is hidden on read-only clusters and asks
//! for a typed name on production clusters (uninstalling with CRDs always does). The writes run
//! in [`Olm`]: closing a dialog doesn't cancel them.

use std::sync::Arc;

use gpui::{
    AnyElement, App, AppContext as _, Context, Entity, FocusHandle, Focusable, FontWeight,
    IntoElement, Render, SharedString, Subscription, Window, div, prelude::*, px,
};
use gpui_component::WindowExt as _;
use gpui_component::button::Button as MenuButton;
use gpui_component::input::{Input, InputEvent, InputState};
use gpui_component::menu::{DropdownMenu as _, PopupMenuItem};
use kubyl_core::actions::OpenView;
use kubyl_core::{
    ClusterId, Gvr, Notification, NotificationCenter, ResourceRef, ViewKind, ViewRequest,
};
use kubyl_kube::ConnectionManager;
use kubyl_resources::{ResourceStores, StoreHandle, StoreKey};
use kubyl_ui::{ActiveColors, Button, Colors, Icon, IconName, ProdBadge, fonts, h_flex, u, v_flex};

use crate::helm::present::Command;
use crate::olm::hub::Package;
use crate::olm::join::Operator;
use crate::olm::model::{self, Approval, Csv, InstallMode, InstallPlan, OwnedCrd};
use crate::olm::ops::{self, GroupStep, InstallChoice, Target, UninstallSpec};
use crate::olm::review::{CrdStatus, Level, Scope};
use crate::service::{Olm, OlmLease, ReviewState};
use crate::widgets;

fn open<V: Render>(
    view: Entity<V>,
    width: f32,
    focus: Option<FocusHandle>,
    window: &mut Window,
    cx: &mut App,
) {
    let colors = cx.colors().clone();
    window.open_dialog(cx, move |dialog, _, _| {
        dialog
            .w(px(width))
            .margin_top(px(60.0))
            .p_0()
            .bg(colors.panel)
            .close_button(false)
            // Enter in an input would close the dialog first; the views submit themselves.
            .on_ok(|_, _, _| false)
            // As content, not a child, so presses on the footer reach its buttons.
            .content({
                let view = view.clone();
                move |content, _, _| content.child(view.clone())
            })
    });
    if let Some(focus) = focus {
        window.focus(&focus, cx);
    }
}

fn header(
    lead: AnyElement,
    title: impl Into<SharedString>,
    extra: Vec<AnyElement>,
    colors: &Colors,
) -> impl IntoElement {
    h_flex()
        .gap(u(10.0))
        .px(u(16.0))
        .py(u(14.0))
        .border_b_1()
        .border_color(colors.border_variant)
        .child(lead)
        .child(
            div()
                .flex_1()
                .min_w_0()
                .truncate()
                .font_weight(FontWeight::SEMIBOLD)
                .child(title.into()),
        )
        .children(extra)
}

fn footer(note: Option<AnyElement>, buttons: Vec<AnyElement>, colors: &Colors) -> impl IntoElement {
    h_flex()
        .gap(u(8.0))
        .px(u(16.0))
        .py(u(12.0))
        .border_t_1()
        .border_color(colors.border_variant)
        .children(note)
        .child(div().flex_1())
        .children(buttons)
}

fn close_button(label: &'static str) -> AnyElement {
    Button::new("operators-dialog-close")
        .ghost()
        .label(label)
        .on_click(|_, window, cx| window.close_dialog(cx))
        .into_any_element()
}

fn error_line(error: &Option<String>, colors: &Colors) -> Option<AnyElement> {
    error.as_ref().map(|e| {
        div()
            .text_size(u(12.0))
            .text_color(colors.red)
            .child(e.clone())
            .into_any_element()
    })
}

fn input_box(input: &Entity<InputState>, mono: bool, colors: &Colors) -> impl IntoElement {
    div()
        .h(u(28.0))
        .px(u(8.0))
        .flex()
        .items_center()
        .rounded(u(5.0))
        .bg(colors.input_background)
        .border_1()
        .border_color(colors.border)
        .when(mono, |this| this.font_family(fonts::MONO))
        .child(
            Input::new(input)
                .appearance(false)
                .text_size(u(if mono { 12.5 } else { 13.0 })),
        )
}

/// A labelled row of the install form.
fn form_row(label: &'static str, content: impl IntoElement, colors: &Colors) -> impl IntoElement {
    h_flex()
        .items_start()
        .gap(u(12.0))
        .child(
            div()
                .flex_none()
                .w(u(104.0))
                .pt(u(5.0))
                .text_size(u(12.0))
                .text_color(colors.text_dim)
                .child(label),
        )
        .child(div().flex_1().min_w_0().child(content))
}

fn radio(
    id: &'static str,
    on: bool,
    label: impl Into<SharedString>,
    detail: impl Into<SharedString>,
    colors: &Colors,
) -> gpui::Stateful<gpui::Div> {
    h_flex()
        .id(id)
        .items_start()
        .gap(u(10.0))
        .p(u(9.0))
        .rounded(u(7.0))
        .border_1()
        .border_color(if on { colors.accent } else { colors.border })
        .when(on, |this| this.bg(colors.selection))
        .cursor_pointer()
        .child(
            div()
                .flex_none()
                .mt(u(2.0))
                .size(u(14.0))
                .rounded_full()
                .border_1()
                .border_color(if on { colors.accent } else { colors.text_faint })
                .flex()
                .items_center()
                .justify_center()
                .when(on, |this| {
                    this.child(div().size(u(7.0)).rounded_full().bg(colors.accent))
                }),
        )
        .child(
            v_flex()
                .min_w_0()
                .child(div().text_size(u(12.5)).child(label.into()))
                .child(
                    div()
                        .text_size(u(11.5))
                        .text_color(colors.text_dim)
                        .child(detail.into()),
                ),
        )
}

fn checkbox(
    id: &'static str,
    on: bool,
    label: impl Into<SharedString>,
    colors: &Colors,
) -> gpui::Stateful<gpui::Div> {
    h_flex()
        .id(id)
        .gap(u(8.0))
        .cursor_pointer()
        .child(
            div()
                .flex_none()
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
                }),
        )
        .child(div().text_size(u(12.5)).child(label.into()))
}

fn typed_row(what: &str, input: &Entity<InputState>, colors: &Colors) -> AnyElement {
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
                        .child(what.to_string()),
                )
                .child("to confirm."),
        )
        .child(input_box(input, true, colors))
        .into_any_element()
}

fn production(cluster: &ClusterId, cx: &App) -> bool {
    ConnectionManager::try_global(cx).is_some_and(|m| m.read(cx).caps(cluster).production)
}

fn read_only(cluster: &ClusterId, cx: &App) -> bool {
    ConnectionManager::try_global(cx).is_some_and(|m| m.read(cx).caps(cluster).read_only)
}

fn cluster_name(cluster: &ClusterId, cx: &App) -> String {
    ConnectionManager::try_global(cx)
        .map(|m| m.read(cx).display_name(cluster).to_string())
        .unwrap_or_else(|| cluster.to_string())
}

fn client(cluster: &ClusterId, cx: &App) -> Option<kube::Client> {
    ConnectionManager::try_global(cx)?.read(cx).client(cluster)
}

/// Refuses writes on read-only clusters (the buttons are hidden; keys may still ask).
fn guard(cluster: &ClusterId, cx: &mut App) -> bool {
    if read_only(cluster, cx) {
        NotificationCenter::push(
            cx,
            Notification::error("This cluster is read-only in Kubyl."),
        );
        return false;
    }
    true
}

fn typed_input(placeholder: String, window: &mut Window, cx: &mut App) -> Entity<InputState> {
    cx.new(|cx| InputState::new(window, cx).placeholder(placeholder))
}

// ----- Install -----

/// Opens the install dialog for an OperatorHub package.
pub fn open_install(cluster: ClusterId, package: Arc<Package>, window: &mut Window, cx: &mut App) {
    if !guard(&cluster, cx) {
        return;
    }
    let view = cx.new(|cx| InstallDialog::new(cluster, package, window, cx));
    let focus = view.read(cx).focus.clone();
    open(view, 600.0, Some(focus), window, cx);
}

struct InstallDialog {
    cluster: ClusterId,
    package: Arc<Package>,
    channel: String,
    /// `None`: the channel head.
    version: Option<String>,
    all_namespaces: bool,
    namespace: Entity<InputState>,
    approval: Approval,
    typed: Entity<InputState>,
    error: Option<String>,
    busy: bool,
    focus: FocusHandle,
    _lease: Option<OlmLease>,
    _subscriptions: Vec<Subscription>,
}

impl InstallDialog {
    fn new(
        cluster: ClusterId,
        package: Arc<Package>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Self {
        let head = package.head().cloned();
        let modes = head
            .as_ref()
            .map(|c| c.install_modes.clone())
            .unwrap_or_default();
        let suggested = head
            .as_ref()
            .and_then(|c| c.suggested_namespace.clone())
            .unwrap_or_else(|| package.name.clone());
        let namespace = cx.new(|cx| {
            let mut state = InputState::new(window, cx).placeholder("namespace");
            state.set_value(suggested, window, cx);
            state
        });
        let typed = typed_input(cluster_name(&cluster, cx), window, cx);
        let subscriptions =
            [&namespace, &typed]
                .into_iter()
                .map(|input| {
                    cx.subscribe_in(input, window, |this, _, event: &InputEvent, window, cx| {
                        match event {
                            InputEvent::Change => {
                                this.error = None;
                                cx.notify();
                            }
                            InputEvent::PressEnter { .. } => this.submit(window, cx),
                            _ => {}
                        }
                    })
                })
                .collect();
        let lease = Olm::watch(&cluster, cx);
        if let Some(olm) = Olm::global(cx) {
            cx.observe(&olm, |_, _, cx| cx.notify()).detach();
        }
        Self {
            channel: package.default_channel.clone(),
            version: None,
            all_namespaces: modes.contains(&InstallMode::AllNamespaces),
            namespace,
            approval: Approval::Automatic,
            typed,
            error: None,
            busy: false,
            focus: cx.focus_handle(),
            cluster,
            package,
            _lease: lease,
            _subscriptions: subscriptions,
        }
    }

    fn choice(&self, cx: &App) -> InstallChoice {
        InstallChoice {
            package: self.package.name.clone(),
            catalog: self.package.catalog.clone(),
            catalog_namespace: self.package.catalog_namespace.clone(),
            channel: self.channel.clone(),
            starting_csv: self.version.clone(),
            approval: self.approval,
            target: if self.all_namespaces {
                Target::AllNamespaces
            } else {
                Target::Namespace(self.namespace.read(cx).value().trim().to_string())
            },
        }
    }

    fn plan(&self, cx: &App) -> Result<ops::InstallSteps, String> {
        let channel = self
            .package
            .channel(&self.channel)
            .ok_or("The channel is gone from the catalog.")?;
        let snapshot = Olm::global(cx)
            .and_then(|o| o.read(cx).snapshot(&self.cluster, cx))
            .ok_or("OLM isn't loaded yet.")?;
        if snapshot.loading {
            return Err("Loading the cluster's OperatorGroups…".into());
        }
        let namespaces = ConnectionManager::try_global(cx)
            .map(|m| m.read(cx).namespaces(&self.cluster).names)
            .unwrap_or_default();
        ops::plan_install(
            &self.choice(cx),
            &channel.install_modes,
            &snapshot.groups,
            &namespaces,
            &snapshot.subscriptions,
        )
    }

    fn typed_ok(&self, cx: &App) -> bool {
        !production(&self.cluster, cx)
            || self.typed.read(cx).value().trim() == cluster_name(&self.cluster, cx)
    }

    fn submit(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.busy {
            return;
        }
        let steps = match self.plan(cx) {
            Ok(steps) => steps,
            Err(err) => {
                self.error = Some(err);
                cx.notify();
                return;
            }
        };
        if !self.typed_ok(cx) {
            self.error = Some("Type the cluster's name to install on a production cluster.".into());
            cx.notify();
            return;
        }
        let (Some(client), Some(olm)) = (client(&self.cluster, cx), Olm::global(cx)) else {
            self.error = Some("The cluster isn't connected.".into());
            cx.notify();
            return;
        };
        self.busy = true;
        let choice = self.choice(cx);
        let display = self.package.display_name.clone();
        let namespace = steps.namespace.clone();
        let select = format!("sub:{}/{}", steps.namespace, steps.subscription);
        olm.update(cx, |olm, cx| {
            olm.run(
                format!("install:{namespace}/{}", choice.package),
                ops::install(client, choice, steps),
                format!("Subscribed to {display} in {namespace}: OLM installs it now."),
                cx,
            )
            .detach();
        });
        window.close_dialog(cx);
        crate::view::open(
            &self.cluster,
            crate::view::Pending {
                tab: Some(crate::view::SubTab::Installed),
                select: Some(select),
            },
            window,
            cx,
        );
    }

    fn channel_menu(&self, colors: &Colors, cx: &mut Context<Self>) -> AnyElement {
        let weak = cx.entity().downgrade();
        let channels: Vec<(String, bool)> = self
            .package
            .channels
            .iter()
            .map(|c| (c.name.clone(), c.name == self.package.default_channel))
            .collect();
        let current = self.channel.clone();
        let label = if current == self.package.default_channel {
            format!("{current} (default)")
        } else {
            current.clone()
        };
        MenuButton::new("install-channel")
            .outline()
            .compact()
            .child(dropdown_label(label, colors))
            .dropdown_menu(move |mut menu, _, _| {
                for (name, default) in &channels {
                    let weak = weak.clone();
                    let value = name.clone();
                    let label = if *default {
                        format!("{name} (default)")
                    } else {
                        name.clone()
                    };
                    menu = menu.item(
                        PopupMenuItem::new(label)
                            .checked(*name == current)
                            .on_click(move |_, _, cx| {
                                weak.update(cx, |this, cx| {
                                    this.channel = value.clone();
                                    this.version = None;
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

    fn version_menu(&self, colors: &Colors, cx: &mut Context<Self>) -> AnyElement {
        let weak = cx.entity().downgrade();
        let channel = self.package.channel(&self.channel).cloned();
        let head_version = channel
            .as_ref()
            .and_then(|c| c.version.clone())
            .unwrap_or_default();
        let entries = channel.map(|c| c.entries).unwrap_or_default();
        let label = match &self.version {
            None => format!("{head_version} · latest"),
            Some(csv) => entries
                .iter()
                .find(|e| &e.csv == csv)
                .and_then(|e| e.version.clone())
                .unwrap_or_else(|| csv.clone()),
        };
        let current = self.version.clone();
        MenuButton::new("install-version")
            .outline()
            .compact()
            .child(dropdown_label(label, colors))
            .dropdown_menu(move |mut menu, _, _| {
                menu = menu.max_h(px(360.0)).scrollable(true);
                for (index, entry) in entries.iter().enumerate() {
                    let weak = weak.clone();
                    let csv = (index > 0).then(|| entry.csv.clone());
                    let label = match (&entry.version, index) {
                        (Some(v), 0) => format!("{v} · latest"),
                        (Some(v), _) => v.clone(),
                        (None, _) => entry.csv.clone(),
                    };
                    menu = menu.item(PopupMenuItem::new(label).checked(csv == current).on_click(
                        move |_, _, cx| {
                            weak.update(cx, |this, cx| {
                                this.version = csv.clone();
                                cx.notify();
                            })
                            .ok();
                        },
                    ));
                }
                menu
            })
            .into_any_element()
    }
}

fn dropdown_label(label: String, colors: &Colors) -> impl IntoElement {
    h_flex()
        .gap(u(6.0))
        .text_size(u(12.5))
        .text_color(colors.text)
        .child(label)
        .child(Icon::new(IconName::ChevronDown).size(11.0))
}

impl Focusable for InstallDialog {
    fn focus_handle(&self, _: &App) -> FocusHandle {
        self.focus.clone()
    }
}

impl Render for InstallDialog {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let colors = cx.colors().clone();
        let package = self.package.clone();
        let channel = package.channel(&self.channel).cloned();
        let modes = channel
            .as_ref()
            .map(|c| c.install_modes.clone())
            .unwrap_or_default();
        let plan = self.plan(cx);
        let prod = production(&self.cluster, cx);
        let global_ns = Olm::global(cx)
            .and_then(|o| o.read(cx).snapshot(&self.cluster, cx))
            .and_then(|s| {
                s.groups
                    .iter()
                    .filter(|g| g.is_global())
                    .map(|g| (g.namespace.clone(), g.name.clone()))
                    .min_by_key(|(ns, _)| (ns != "openshift-operators", ns.clone()))
            });
        let all_detail = match (&global_ns, modes.contains(&InstallMode::AllNamespaces)) {
            (_, false) => "Not supported by this operator.".to_string(),
            (Some((ns, og)), true) => {
                format!("Into {ns} with the OperatorGroup {og}; watches every namespace.")
            }
            (None, true) => {
                "Into operators with a new global OperatorGroup; watches every namespace."
                    .to_string()
            }
        };
        let own_detail = if modes.contains(&InstallMode::OwnNamespace) {
            "The operator watches only the namespace it's installed in."
        } else {
            "Not supported by this operator."
        };
        let summary = match &plan {
            Ok(steps) => {
                let mut lines: Vec<AnyElement> = Vec::new();
                let line = |kind: &str, name: String, detail: String| {
                    h_flex()
                        .gap(u(8.0))
                        .text_size(u(12.5))
                        .child(Icon::new(IconName::Plus).size(12.0).color(colors.green))
                        .child(kind.to_string())
                        .child(
                            div()
                                .font_family(fonts::MONO)
                                .text_size(u(11.5))
                                .child(name),
                        )
                        .child(div().text_color(colors.text_dim).child(detail))
                        .into_any_element()
                };
                if steps.create_namespace {
                    lines.push(line("Namespace", steps.namespace.clone(), String::new()));
                }
                match &steps.group {
                    GroupStep::Create { name, global } => lines.push(line(
                        "OperatorGroup",
                        format!("{}/{name}", steps.namespace),
                        if *global {
                            "· all namespaces".into()
                        } else {
                            format!("· targets {}", steps.namespace)
                        },
                    )),
                    GroupStep::Existing(name) => lines.push(
                        h_flex()
                            .gap(u(8.0))
                            .text_size(u(12.5))
                            .text_color(colors.text_muted)
                            .child(Icon::new(IconName::Check).size(12.0).color(colors.text_dim))
                            .child("Uses OperatorGroup")
                            .child(
                                div()
                                    .font_family(fonts::MONO)
                                    .text_size(u(11.5))
                                    .child(format!("{}/{name}", steps.namespace)),
                            )
                            .into_any_element(),
                    ),
                }
                let version = self
                    .version
                    .clone()
                    .map(|csv| format!(" · starts at {csv}"))
                    .unwrap_or_default();
                lines.push(line(
                    "Subscription",
                    format!("{}/{}", steps.namespace, steps.subscription),
                    format!("· {} · {}{version}", self.channel, self.approval.label()),
                ));
                v_flex()
                    .gap(u(5.0))
                    .p(u(10.0))
                    .rounded(u(7.0))
                    .bg(colors.subheader_background)
                    .border_1()
                    .border_color(colors.border)
                    .child(
                        div()
                            .text_size(u(11.0))
                            .font_weight(FontWeight::SEMIBOLD)
                            .text_color(colors.text_dim)
                            .child("WHAT KUBYL CREATES"),
                    )
                    .children(lines)
                    .child(
                        div()
                            .text_size(u(12.0))
                            .text_color(colors.text_dim)
                            .child(format!(
                                "OLM then installs {} CRDs, the operator's Deployment and its RBAC (the install plan lists them).",
                                channel.as_ref().map_or(0, |c| c.owned.len())
                            )),
                    )
                    .into_any_element()
            }
            Err(err) => div()
                .p(u(10.0))
                .rounded(u(7.0))
                .bg(colors.red.opacity(0.1))
                .border_1()
                .border_color(colors.red.opacity(0.35))
                .text_size(u(12.5))
                .text_color(colors.red)
                .child(err.clone())
                .into_any_element(),
        };
        let can_install = plan.is_ok() && self.typed_ok(cx) && !self.busy;
        let deprecated = package.deprecated.clone();
        let approval = |label: &'static str, value: Approval, cx: &mut Context<Self>| {
            widgets::toggle_chip(
                SharedString::from(format!("approval-{label}")),
                label,
                self.approval == value,
                cx.listener(move |this, _, _, cx| {
                    this.approval = value;
                    cx.notify();
                }),
            )
        };
        let tile = widgets::tile(package.initials(), &package.name, None, 26.0, &colors);
        v_flex()
            .track_focus(&self.focus)
            .text_color(colors.text)
            .text_size(u(13.0))
            .child(header(
                tile,
                format!("Install {}", package.display_name),
                [
                    Some(
                        div()
                            .text_size(u(12.0))
                            .text_color(colors.text_dim)
                            .child(package.catalog_display.clone())
                            .into_any_element(),
                    ),
                    prod.then(|| ProdBadge.into_any_element()),
                ]
                .into_iter()
                .flatten()
                .collect(),
                &colors,
            ))
            .child(
                v_flex()
                    .p(u(16.0))
                    .gap(u(14.0))
                    .when_some(deprecated, |this, text| {
                        this.child(widgets::note(
                            IconName::TriangleAlert,
                            colors.yellow,
                            text,
                            &colors,
                        ))
                    })
                    .child(form_row(
                        "Channel",
                        h_flex()
                            .gap(u(8.0))
                            .child(self.channel_menu(&colors, cx))
                            .child(self.version_menu(&colors, cx)),
                        &colors,
                    ))
                    .child(form_row(
                        "Install mode",
                        v_flex()
                            .gap(u(6.0))
                            .child(
                                radio(
                                    "install-all",
                                    self.all_namespaces,
                                    "All namespaces",
                                    all_detail,
                                    &colors,
                                )
                                .on_click(cx.listener(|this, _, _, cx| {
                                    this.all_namespaces = true;
                                    this.error = None;
                                    cx.notify();
                                })),
                            )
                            .child(
                                radio(
                                    "install-own",
                                    !self.all_namespaces,
                                    "A specific namespace",
                                    own_detail,
                                    &colors,
                                )
                                .on_click(cx.listener(|this, _, _, cx| {
                                    this.all_namespaces = false;
                                    this.error = None;
                                    cx.notify();
                                })),
                            ),
                        &colors,
                    ))
                    .when(!self.all_namespaces, |this| {
                        this.child(form_row(
                            "Namespace",
                            input_box(&self.namespace, true, &colors),
                            &colors,
                        ))
                    })
                    .child(form_row(
                        "Approval",
                        v_flex()
                            .gap(u(5.0))
                            .child(
                                h_flex()
                                    .gap(u(6.0))
                                    .child(approval("Automatic", Approval::Automatic, cx))
                                    .child(approval("Manual", Approval::Manual, cx)),
                            )
                            .child(
                                div()
                                    .text_size(u(11.5))
                                    .text_color(colors.text_dim)
                                    .child(match self.approval {
                                        Approval::Automatic => "Upgrades in the channel install as soon as the catalog has them.",
                                        Approval::Manual => "Every install and upgrade waits for someone to approve its install plan.",
                                    }),
                            ),
                        &colors,
                    ))
                    .child(summary)
                    .when(prod, |this| {
                        this.child(typed_row(&cluster_name(&self.cluster, cx), &self.typed, &colors))
                    })
                    .children(error_line(&self.error, &colors)),
            )
            .child(footer(
                None,
                vec![
                    close_button("Cancel"),
                    Button::new("install-submit")
                        .primary()
                        .icon(IconName::Download)
                        .label(if self.busy { "Installing…" } else { "Install" })
                        .disabled(!can_install)
                        .on_click(cx.listener(|this, _, window, cx| this.submit(window, cx)))
                        .into_any_element(),
                ],
                &colors,
            ))
    }
}

// ----- Review and approve -----

/// Opens the review of a pending install plan (CRD diffs, RBAC, compatibility) with Approve.
pub fn open_review(cluster: ClusterId, plan: Arc<InstallPlan>, window: &mut Window, cx: &mut App) {
    let view = cx.new(|cx| ReviewDialog::new(cluster, plan, window, cx));
    let focus = view.read(cx).focus.clone();
    open(view, 1040.0, Some(focus), window, cx);
}

struct ReviewDialog {
    cluster: ClusterId,
    plan: Arc<InstallPlan>,
    selected: usize,
    side_by_side: bool,
    typed: Entity<InputState>,
    error: Option<String>,
    focus: FocusHandle,
    _lease: Option<OlmLease>,
    _subscriptions: Vec<Subscription>,
}

impl ReviewDialog {
    fn new(
        cluster: ClusterId,
        plan: Arc<InstallPlan>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Self {
        let operator = Self::operator_of(&cluster, &plan, cx);
        let typed = typed_input(
            operator
                .as_ref()
                .map(|o| o.typed_name())
                .unwrap_or_else(|| plan.name.clone()),
            window,
            cx,
        );
        let subscriptions =
            vec![
                cx.subscribe_in(&typed, window, |this, _, event: &InputEvent, window, cx| {
                    match event {
                        InputEvent::Change => {
                            this.error = None;
                            cx.notify();
                        }
                        InputEvent::PressEnter { .. } => this.approve(window, cx),
                        _ => {}
                    }
                }),
            ];
        let lease = Olm::watch(&cluster, cx);
        if let Some(olm) = Olm::global(cx) {
            cx.observe(&olm, |_, _, cx| cx.notify()).detach();
        }
        Self {
            cluster,
            plan,
            selected: 0,
            side_by_side: false,
            typed,
            error: None,
            focus: cx.focus_handle(),
            _lease: lease,
            _subscriptions: subscriptions,
        }
    }

    fn operator_of(cluster: &ClusterId, plan: &InstallPlan, cx: &App) -> Option<Operator> {
        let snapshot = Olm::global(cx)?.read(cx).snapshot(cluster, cx)?;
        snapshot
            .operators
            .iter()
            .find(|o| {
                o.subscription.as_ref().is_some_and(|s| {
                    s.namespace == plan.namespace
                        && (plan.subscriptions.contains(&s.name)
                            || s.install_plan.as_deref() == Some(&plan.name))
                })
            })
            .cloned()
    }

    /// The plan as the watch has it now (approved meanwhile…).
    fn current_plan(&self, cx: &App) -> Arc<InstallPlan> {
        Olm::global(cx)
            .and_then(|o| o.read(cx).snapshot(&self.cluster, cx))
            .and_then(|s| s.plan(&self.plan.namespace, &self.plan.name).cloned())
            .unwrap_or_else(|| self.plan.clone())
    }

    fn typed_name(&self, cx: &App) -> String {
        Self::operator_of(&self.cluster, &self.plan, cx)
            .map(|o| o.typed_name())
            .unwrap_or_else(|| self.plan.name.clone())
    }

    fn typed_ok(&self, cx: &App) -> bool {
        !production(&self.cluster, cx) || self.typed.read(cx).value().trim() == self.typed_name(cx)
    }

    fn approve(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if !guard(&self.cluster, cx) {
            return;
        }
        if !self.typed_ok(cx) {
            self.error =
                Some("Type the operator's name to approve on a production cluster.".into());
            cx.notify();
            return;
        }
        let (Some(client), Some(olm)) = (client(&self.cluster, cx), Olm::global(cx)) else {
            self.error = Some("The cluster isn't connected.".into());
            cx.notify();
            return;
        };
        let plan = self.plan.clone();
        olm.update(cx, |olm, cx| {
            olm.run(
                format!("plan:{}", plan.key()),
                ops::approve(client, plan.namespace.clone(), plan.name.clone()),
                format!(
                    "Approved {}: OLM installs it now.",
                    plan.csv_names.join(", ")
                ),
                cx,
            )
            .detach();
        });
        window.close_dialog(cx);
    }
}

impl Focusable for ReviewDialog {
    fn focus_handle(&self, _: &App) -> FocusHandle {
        self.focus.clone()
    }
}

impl Render for ReviewDialog {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let colors = cx.colors().clone();
        let plan = self.current_plan(cx);
        let operator = Self::operator_of(&self.cluster, &plan, cx);
        let installed = operator.as_ref().and_then(|o| o.csv.clone());
        let state = match Olm::global(cx) {
            Some(olm) => olm.update(cx, |olm, cx| {
                olm.review(&self.cluster, &plan, installed.clone(), cx)
            }),
            None => ReviewState::Loading,
        };
        let from = installed
            .as_ref()
            .and_then(|c| c.version.clone())
            .unwrap_or_else(|| "not installed".into());
        let to = match &state {
            ReviewState::Ready(review) => review.version.clone(),
            ReviewState::Loading => None,
        }
        .or_else(|| operator.as_ref().and_then(|o| o.upgrade_to.clone()))
        .unwrap_or_else(|| plan.csv_names.join(", "));
        let name = operator
            .as_ref()
            .map(|o| o.display_name())
            .unwrap_or_else(|| plan.csv_names.join(", "));
        let prod = production(&self.cluster, cx);
        let lead = widgets::tile(
            crate::olm::join::initials(&name),
            &name,
            installed.as_ref().and_then(|c| widgets::csv_icon(&c.icon)),
            26.0,
            &colors,
        );
        let versions = h_flex()
            .gap(u(6.0))
            .font_family(fonts::MONO)
            .text_size(u(12.5))
            .child(from)
            .child(div().text_color(colors.text_dim).child("→"))
            .child(div().text_color(colors.green).child(to))
            .into_any_element();
        let phase = div()
            .text_size(u(12.0))
            .text_color(colors.text_dim)
            .child(format!("install plan {} · {}", plan.key(), plan.phase))
            .into_any_element();
        let body: AnyElement = match &state {
            ReviewState::Loading => div()
                .h(u(440.0))
                .flex()
                .items_center()
                .justify_center()
                .text_color(colors.text_dim)
                .child("Reading the plan's bundle and the live CRDs…")
                .into_any_element(),
            ReviewState::Ready(review) => {
                let selected = self.selected.min(review.crds.len().saturating_sub(1));
                let crd_list = v_flex()
                    .id("review-crds")
                    .flex_none()
                    .w(u(260.0))
                    .h_full()
                    .overflow_y_scroll()
                    .p(u(10.0))
                    .gap(u(4.0))
                    .border_r_1()
                    .border_color(colors.border_variant)
                    .child(
                        div()
                            .px(u(4.0))
                            .text_size(u(11.0))
                            .font_weight(FontWeight::SEMIBOLD)
                            .text_color(colors.text_dim)
                            .child(format!(
                                "CRDS · {} OF {} CHANGE",
                                review.changed_crds(),
                                review.crds.len()
                            )),
                    )
                    .children(review.crds.iter().enumerate().map(|(i, crd)| {
                        let on = i == selected;
                        let color = match crd.status {
                            CrdStatus::New => colors.green,
                            CrdStatus::Changed => colors.text,
                            CrdStatus::Unchanged => colors.text_dim,
                            CrdStatus::Unknown => colors.red,
                        };
                        v_flex()
                            .id(("review-crd", i))
                            .px(u(10.0))
                            .py(u(6.0))
                            .rounded(u(6.0))
                            .cursor_pointer()
                            .when(on, |this| {
                                this.bg(colors.selection)
                                    .border_1()
                                    .border_color(colors.accent)
                            })
                            .child(
                                div()
                                    .truncate()
                                    .font_family(fonts::MONO)
                                    .text_size(u(11.5))
                                    .text_color(color)
                                    .child(crd.name.clone()),
                            )
                            .child(
                                div()
                                    .truncate()
                                    .text_size(u(11.5))
                                    .text_color(colors.text_dim)
                                    .child(crd.summary.clone()),
                            )
                            .on_click(cx.listener(move |this, _, _, cx| {
                                this.selected = i;
                                cx.notify();
                            }))
                    }))
                    .child(
                        div()
                            .pt(u(10.0))
                            .px(u(4.0))
                            .text_size(u(11.0))
                            .font_weight(FontWeight::SEMIBOLD)
                            .text_color(colors.text_dim)
                            .child("ALSO IN THE PLAN"),
                    )
                    .children(
                        review
                            .other_steps
                            .iter()
                            .take(40)
                            .map(|(kind, name, action)| {
                                h_flex()
                                    .px(u(4.0))
                                    .gap(u(6.0))
                                    .text_size(u(11.5))
                                    .child(
                                        div()
                                            .flex_none()
                                            .text_color(colors.text_dim)
                                            .child(kind.clone()),
                                    )
                                    .child(
                                        div()
                                            .flex_1()
                                            .min_w_0()
                                            .truncate()
                                            .font_family(fonts::MONO)
                                            .child(name.clone()),
                                    )
                                    .child(
                                        div()
                                            .flex_none()
                                            .text_color(if action == "create" {
                                                colors.green
                                            } else {
                                                colors.accent
                                            })
                                            .child(action.clone()),
                                    )
                            }),
                    )
                    .child(
                        div()
                            .pt(u(8.0))
                            .px(u(4.0))
                            .text_size(u(11.0))
                            .text_color(colors.text_dim)
                            .child("From the bundle OLM unpacked, compared with the live CRDs."),
                    );
                let diff: AnyElement = match review.crds.get(selected) {
                    None => widgets::empty("The plan changes no CRDs.", &colors),
                    Some(crd) if crd.status == CrdStatus::Unknown => {
                        widgets::empty(crd.summary.clone(), &colors)
                    }
                    Some(crd) if crd.diff.is_empty() => {
                        widgets::empty("The CRD doesn't change.", &colors)
                    }
                    Some(crd) => div()
                        .id("review-diff")
                        .size_full()
                        .overflow_y_scroll()
                        .child(kubyl_yaml::diff_view(&crd.diff, self.side_by_side, &colors))
                        .into_any_element(),
                };
                let diff_title = review
                    .crds
                    .get(selected)
                    .map(|c| c.name.clone())
                    .unwrap_or_default();
                let middle = v_flex()
                    .flex_1()
                    .min_w_0()
                    .h_full()
                    .child(
                        h_flex()
                            .flex_none()
                            .h(u(34.0))
                            .px(u(12.0))
                            .gap(u(8.0))
                            .border_b_1()
                            .border_color(colors.border_variant)
                            .text_size(u(12.0))
                            .child(div().font_family(fonts::MONO).child(diff_title))
                            .child(
                                div()
                                    .text_color(colors.text_dim)
                                    .child("· spec, live → bundle"),
                            )
                            .child(div().flex_1())
                            .child(widgets::toggle_chip(
                                "review-unified",
                                "unified",
                                !self.side_by_side,
                                cx.listener(|this, _, _, cx| {
                                    this.side_by_side = false;
                                    cx.notify();
                                }),
                            ))
                            .child(widgets::toggle_chip(
                                "review-side",
                                "side by side",
                                self.side_by_side,
                                cx.listener(|this, _, _, cx| {
                                    this.side_by_side = true;
                                    cx.notify();
                                }),
                            )),
                    )
                    .child(div().flex_1().min_h_0().child(diff));
                let rbac: Vec<AnyElement> = if review.rbac.is_empty() {
                    vec![
                        div()
                            .text_size(u(12.0))
                            .text_color(colors.text_dim)
                            .child("No permission changes.")
                            .into_any_element(),
                    ]
                } else {
                    review
                        .rbac
                        .iter()
                        .take(30)
                        .map(|change| {
                            h_flex()
                                .items_start()
                                .gap(u(8.0))
                                .py(u(3.0))
                                .border_b_1()
                                .border_color(colors.border_variant)
                                .text_size(u(12.0))
                                .child(
                                    div()
                                        .flex_none()
                                        .w(u(12.0))
                                        .font_weight(FontWeight::SEMIBOLD)
                                        .text_color(if change.added {
                                            colors.green
                                        } else {
                                            colors.red
                                        })
                                        .child(if change.added { "+" } else { "−" }),
                                )
                                .child(
                                    v_flex()
                                        .flex_1()
                                        .min_w_0()
                                        .child(
                                            div()
                                                .font_family(fonts::MONO)
                                                .text_size(u(11.5))
                                                .child(format!(
                                                    "{} {}",
                                                    change.verbs.join(", "),
                                                    change.target
                                                )),
                                        )
                                        .child(
                                            div()
                                                .text_size(u(11.0))
                                                .text_color(colors.text_dim)
                                                .child(format!(
                                                    "{} · {}",
                                                    match change.scope {
                                                        Scope::Cluster => "cluster-wide",
                                                        Scope::Namespaced => "watched namespaces",
                                                    },
                                                    change.service_account
                                                )),
                                        ),
                                )
                                .into_any_element()
                        })
                        .collect()
                };
                let notes: Vec<AnyElement> = review
                    .notes
                    .iter()
                    .map(|n| {
                        let (icon, color) = match n.level {
                            Level::Ok => (IconName::CircleCheck, colors.green),
                            Level::Warning => (IconName::TriangleAlert, colors.yellow),
                            Level::Blocker => (IconName::CircleX, colors.red),
                        };
                        widgets::note(icon, color, n.text.clone(), &colors)
                    })
                    .chain(review.problems.iter().map(|p| {
                        widgets::note(IconName::TriangleAlert, colors.yellow, p.clone(), &colors)
                    }))
                    .collect();
                let right = v_flex()
                    .id("review-right")
                    .flex_none()
                    .w(u(330.0))
                    .h_full()
                    .overflow_y_scroll()
                    .p(u(14.0))
                    .gap(u(12.0))
                    .border_l_1()
                    .border_color(colors.border_variant)
                    .child(
                        v_flex()
                            .gap(u(6.0))
                            .child(small_title("RBAC · OPERATOR SERVICE ACCOUNT", &colors))
                            .children(rbac),
                    )
                    .child(
                        v_flex()
                            .gap(u(4.0))
                            .child(small_title("COMPATIBILITY", &colors))
                            .children(if notes.is_empty() {
                                vec![
                                    div()
                                        .text_size(u(12.0))
                                        .text_color(colors.text_dim)
                                        .child("Nothing to check.")
                                        .into_any_element(),
                                ]
                            } else {
                                notes
                            }),
                    );
                h_flex()
                    .h(u(460.0))
                    .items_start()
                    .child(crd_list)
                    .child(middle)
                    .child(right)
                    .into_any_element()
            }
        };
        let can_approve = plan.needs_approval() && !read_only(&self.cluster, cx);
        let busy =
            Olm::global(cx).is_some_and(|o| o.read(cx).is_busy(&format!("plan:{}", plan.key())));
        let plan_ref = ResourceRef::object(
            self.cluster.clone(),
            model::install_plans(),
            Some(plan.namespace.clone()),
            plan.name.clone(),
        );
        let mut buttons = vec![
            Button::new("review-yaml")
                .ghost()
                .icon(IconName::Code)
                .label("Plan YAML")
                .on_click(move |_, window, cx| {
                    window.close_dialog(cx);
                    window.dispatch_action(
                        Box::new(OpenView(ViewRequest::for_resource(
                            ViewKind::Yaml,
                            plan_ref.clone(),
                        ))),
                        cx,
                    );
                })
                .into_any_element(),
            close_button("Close"),
        ];
        if can_approve {
            buttons.push(
                Button::new("review-approve")
                    .primary()
                    .icon(IconName::Check)
                    .label(if busy { "Approving…" } else { "Approve" })
                    .disabled(busy || !self.typed_ok(cx))
                    .on_click(cx.listener(|this, _, window, cx| this.approve(window, cx)))
                    .into_any_element(),
            );
        }
        let note: Option<AnyElement> = if !plan.needs_approval() {
            Some(
                div()
                    .text_size(u(12.0))
                    .text_color(colors.text_dim)
                    .child(if plan.approved {
                        "Approved."
                    } else {
                        "Nothing to approve."
                    })
                    .into_any_element(),
            )
        } else if prod && can_approve {
            Some(
                h_flex()
                    .gap(u(10.0))
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
                                    .child(self.typed_name(cx)),
                            )
                            .child(format!("to approve on {}", cluster_name(&self.cluster, cx))),
                    )
                    .child(
                        div()
                            .w(u(220.0))
                            .child(input_box(&self.typed, true, &colors)),
                    )
                    .into_any_element(),
            )
        } else {
            None
        };
        v_flex()
            .track_focus(&self.focus)
            .text_color(colors.text)
            .text_size(u(13.0))
            .child(header(
                lead,
                format!("Upgrade {name}"),
                [
                    Some(versions),
                    Some(div().flex_1().into_any_element()),
                    Some(phase),
                    prod.then(|| ProdBadge.into_any_element()),
                ]
                .into_iter()
                .flatten()
                .collect(),
                &colors,
            ))
            .child(body)
            .children(
                error_line(&self.error, &colors).map(|e| div().px(u(16.0)).pb(u(8.0)).child(e)),
            )
            .child(footer(note, buttons, &colors))
    }
}

fn small_title(text: &'static str, colors: &Colors) -> impl IntoElement {
    div()
        .text_size(u(11.0))
        .font_weight(FontWeight::SEMIBOLD)
        .text_color(colors.text_dim)
        .child(text)
}

// ----- Uninstall -----

/// Opens the uninstall dialog of an installed operator.
pub fn open_uninstall(cluster: ClusterId, operator: Operator, window: &mut Window, cx: &mut App) {
    if !guard(&cluster, cx) {
        return;
    }
    let view = cx.new(|cx| UninstallDialog::new(cluster, operator, window, cx));
    let focus = view.read(cx).focus.clone();
    open(view, 560.0, Some(focus), window, cx);
}

/// A served CRD: its resource, whether it's namespaced, and a metadata watch of its objects.
type ServedCrd = (Gvr, bool, StoreHandle);

struct UninstallDialog {
    cluster: ClusterId,
    operator: Operator,
    with_crds: bool,
    /// The owned CRDs with a metadata watch of their instances.
    crds: Vec<(OwnedCrd, Option<ServedCrd>)>,
    typed: Entity<InputState>,
    error: Option<String>,
    focus: FocusHandle,
    _subscriptions: Vec<Subscription>,
}

impl UninstallDialog {
    fn new(
        cluster: ClusterId,
        operator: Operator,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Self {
        let typed = typed_input(operator.typed_name(), window, cx);
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
        let discovery =
            ConnectionManager::try_global(cx).and_then(|m| m.read(cx).discovery(&cluster));
        let crds = operator
            .csv
            .as_ref()
            .map(|csv| csv.owned.clone())
            .unwrap_or_default()
            .into_iter()
            .map(|crd| {
                let served = discovery.as_ref().and_then(|d| {
                    kubyl_explorer::catalog::find(d, crd.group(), crd.plural())
                        .map(|info| (info.gvr.clone(), info.namespaced))
                });
                let watch = served.map(|(gvr, namespaced)| {
                    let handle = ResourceStores::acquire(
                        cx,
                        StoreKey::new(cluster.clone(), gvr.clone(), None).metadata(),
                    );
                    cx.observe(handle.entity(), |_, _, cx| cx.notify()).detach();
                    (gvr, namespaced, handle)
                });
                (crd, watch)
            })
            .collect();
        Self {
            cluster,
            operator,
            with_crds: false,
            crds,
            typed,
            error: None,
            focus: cx.focus_handle(),
            _subscriptions: subscriptions,
        }
    }

    fn needs_typed(&self, cx: &App) -> bool {
        self.with_crds || production(&self.cluster, cx)
    }

    fn typed_ok(&self, cx: &App) -> bool {
        !self.needs_typed(cx) || self.typed.read(cx).value().trim() == self.operator.typed_name()
    }

    fn instances(&self, cx: &App) -> usize {
        self.crds
            .iter()
            .filter_map(|(_, w)| w.as_ref())
            .map(|(_, _, h)| h.read(cx).len())
            .sum()
    }

    fn submit(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if !self.typed_ok(cx) {
            self.error = Some("Type the operator's name to confirm.".into());
            cx.notify();
            return;
        }
        let (Some(client), Some(olm)) = (client(&self.cluster, cx), Olm::global(cx)) else {
            self.error = Some("The cluster isn't connected.".into());
            cx.notify();
            return;
        };
        let crds = if self.with_crds {
            let discovery =
                ConnectionManager::try_global(cx).and_then(|m| m.read(cx).discovery(&self.cluster));
            self.crds
                .iter()
                .filter_map(|(crd, watch)| {
                    let (gvr, namespaced, _) = watch.as_ref()?;
                    let info = discovery
                        .as_ref()?
                        .resources
                        .iter()
                        .find(|r| &r.gvr == gvr)?;
                    Some((
                        crd.name.clone(),
                        kubyl_resources::store::api_resource(info),
                        *namespaced,
                    ))
                })
                .collect()
        } else {
            Vec::new()
        };
        let spec = UninstallSpec {
            subscription: self
                .operator
                .subscription
                .as_ref()
                .map(|s| (s.namespace.clone(), s.name.clone())),
            csv: self
                .operator
                .csv
                .as_ref()
                .map(|c| (c.namespace.clone(), c.name.clone())),
            crds,
        };
        let name = self.operator.display_name();
        let (tx, mut rx) = futures::channel::mpsc::unbounded::<String>();
        cx.spawn(async move |_, cx| {
            use futures::StreamExt as _;
            while let Some(line) = rx.next().await {
                cx.update(|cx| NotificationCenter::push(cx, Notification::info(line)));
            }
        })
        .detach();
        olm.update(cx, |olm, cx| {
            olm.run(
                format!("uninstall:{}", self.operator.key),
                ops::uninstall(client, spec, move |line| {
                    tx.unbounded_send(line).ok();
                }),
                format!("Uninstalled {name}."),
                cx,
            )
            .detach();
        });
        window.close_dialog(cx);
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
        let op = &self.operator;
        let name = op.display_name();
        let prod = production(&self.cluster, cx);
        let instances = self.instances(cx);
        let line = |icon: IconName, color, text: String| {
            h_flex()
                .gap(u(8.0))
                .text_size(u(12.5))
                .child(Icon::new(icon).size(12.0).color(color))
                .child(text)
                .into_any_element()
        };
        let mut lines = Vec::new();
        if self.with_crds {
            lines.push(line(
                IconName::Trash,
                colors.red,
                format!(
                    "{instances} instances of its {} kinds, first (the operator handles their finalizers)",
                    self.crds.len()
                ),
            ));
        }
        if let Some(sub) = &op.subscription {
            lines.push(line(
                IconName::Trash,
                colors.red,
                format!("Subscription {}/{}", sub.namespace, sub.name),
            ));
        }
        if let Some(csv) = &op.csv {
            lines.push(line(
                IconName::Trash,
                colors.red,
                format!("ClusterServiceVersion {} (and OLM's copies)", csv.name),
            ));
        }
        if self.with_crds {
            for (crd, _) in &self.crds {
                lines.push(line(
                    IconName::Trash,
                    colors.red,
                    format!("CRD {}", crd.name),
                ));
            }
        }
        lines.push(line(
            IconName::Check,
            colors.text_dim,
            format!(
                "Keeps the namespace {} and its OperatorGroup (other operators may use them).",
                op.namespace()
            ),
        ));
        let crd_rows: Vec<AnyElement> = self
            .crds
            .iter()
            .map(|(crd, watch)| {
                let count = match watch {
                    Some((_, _, handle)) => {
                        let store = handle.read(cx);
                        if store.status().is_settled() {
                            format!("{} instances", store.len())
                        } else {
                            "…".into()
                        }
                    }
                    None => "not served".into(),
                };
                h_flex()
                    .gap(u(8.0))
                    .pl(u(22.0))
                    .text_size(u(12.0))
                    .child(
                        div()
                            .flex_1()
                            .font_family(fonts::MONO)
                            .text_size(u(11.5))
                            .child(crd.name.clone()),
                    )
                    .child(div().text_color(colors.text_dim).child(count))
                    .into_any_element()
            })
            .collect();
        let busy =
            Olm::global(cx).is_some_and(|o| o.read(cx).is_busy(&format!("uninstall:{}", op.key)));
        v_flex()
            .track_focus(&self.focus)
            .text_color(colors.text)
            .text_size(u(13.0))
            .child(header(
                Icon::new(IconName::Trash).size(16.0).color(colors.red).into_any_element(),
                format!("Uninstall {name}"),
                prod.then(|| ProdBadge.into_any_element()).into_iter().collect(),
                &colors,
            ))
            .child(
                v_flex()
                    .p(u(16.0))
                    .gap(u(12.0))
                    .child(
                        checkbox(
                            "uninstall-crds",
                            self.with_crds,
                            "Also delete its CRDs and every instance of them",
                            &colors,
                        )
                        .on_click(cx.listener(|this, _, _, cx| {
                            this.with_crds = !this.with_crds;
                            cx.notify();
                        })),
                    )
                    .when(!crd_rows.is_empty(), |this| this.children(crd_rows))
                    .when(self.with_crds && instances > 0, |this| {
                        this.child(
                            h_flex()
                                .gap(u(10.0))
                                .p(u(10.0))
                                .rounded(u(7.0))
                                .bg(colors.red.opacity(0.12))
                                .border_1()
                                .border_color(colors.red.opacity(0.35))
                                .text_size(u(12.5))
                                .child(Icon::new(IconName::TriangleAlert).size(15.0).color(colors.red))
                                .child(format!(
                                    "Deletes {instances} objects in every namespace, and whatever their operator manages for them (databases, volumes…). This can't be undone."
                                )),
                        )
                    })
                    .child(
                        v_flex()
                            .gap(u(5.0))
                            .p(u(10.0))
                            .rounded(u(7.0))
                            .bg(colors.subheader_background)
                            .border_1()
                            .border_color(colors.border)
                            .child(small_title("WHAT KUBYL DELETES", &colors))
                            .children(lines),
                    )
                    .when(self.needs_typed(cx), |this| {
                        this.child(typed_row(&op.typed_name(), &self.typed, &colors))
                    })
                    .children(error_line(&self.error, &colors)),
            )
            .child(footer(
                None,
                vec![
                    close_button("Cancel"),
                    Button::new("uninstall-submit")
                        .danger()
                        .icon(IconName::Trash)
                        .label(if busy { "Uninstalling…" } else { "Uninstall" })
                        .disabled(busy || !self.typed_ok(cx))
                        .on_click(cx.listener(|this, _, window, cx| this.submit(window, cx)))
                        .into_any_element(),
                ],
                &colors,
            ))
    }
}

// ----- Create an instance -----

/// Opens the YAML editor for a new object of `crd`, with the operator's example for it.
pub fn create_instance(
    cluster: &ClusterId,
    csv: &Csv,
    crd: &OwnedCrd,
    window: &mut Window,
    cx: &mut App,
) {
    if !guard(cluster, cx) {
        return;
    }
    let discovery = ConnectionManager::try_global(cx).and_then(|m| m.read(cx).discovery(cluster));
    let served = discovery.as_ref().and_then(|d| {
        kubyl_explorer::catalog::find(d, crd.group(), crd.plural())
            .map(|i| (i.gvr.clone(), i.namespaced))
    });
    let (gvr, namespaced) = served.unwrap_or_else(|| (crd.gvr(), true));
    // Operators that watch their own namespace only see objects there.
    let namespace = namespaced.then(|| match csv.target_namespaces.as_deref() {
        Some(targets) if !targets.is_empty() && !targets.contains(',') => targets.to_string(),
        _ => kubyl_core::ActiveContext::global(cx)
            .namespace
            .as_ref()
            .map(|n| n.to_string())
            .unwrap_or_else(|| "default".into()),
    });
    let target = ResourceRef::list(cluster.clone(), gvr, namespace.clone());
    let example = csv.examples().into_iter().find(|e| {
        e.get("kind").and_then(|k| k.as_str()) == Some(crd.kind.as_str())
            && e.get("apiVersion")
                .and_then(|v| v.as_str())
                .is_some_and(|v| v.starts_with(crd.group()))
    });
    match example {
        Some(mut example) => {
            if let Some(meta) = example.get_mut("metadata").and_then(|m| m.as_object_mut()) {
                match &namespace {
                    Some(ns) => {
                        meta.insert("namespace".into(), serde_json::Value::String(ns.clone()));
                    }
                    None => {
                        meta.remove("namespace");
                    }
                }
            }
            let text = kubyl_yaml::render::to_yaml(&example);
            kubyl_yaml::open_draft(
                target,
                text,
                Some(format!(
                    "Example from {} (alm-examples). Review it before applying: examples are often minimal.",
                    csv.name
                )),
                window,
                cx,
            );
        }
        None => window.dispatch_action(
            Box::new(OpenView(ViewRequest::for_resource(ViewKind::Yaml, target))),
            cx,
        ),
    }
}

/// Opens a picker of the operator's kinds; the choice opens [`create_instance`].
pub fn open_create_picker(cluster: ClusterId, csv: Arc<Csv>, window: &mut Window, cx: &mut App) {
    if !guard(&cluster, cx) {
        return;
    }
    if csv.owned.len() == 1 {
        create_instance(&cluster, &csv, &csv.owned[0], window, cx);
        return;
    }
    let view = cx.new(|cx| KindPicker {
        cluster,
        csv,
        focus: cx.focus_handle(),
    });
    let focus = view.read(cx).focus.clone();
    open(view, 440.0, Some(focus), window, cx);
}

struct KindPicker {
    cluster: ClusterId,
    csv: Arc<Csv>,
    focus: FocusHandle,
}

impl Focusable for KindPicker {
    fn focus_handle(&self, _: &App) -> FocusHandle {
        self.focus.clone()
    }
}

impl Render for KindPicker {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let colors = cx.colors().clone();
        let examples: Vec<String> = self
            .csv
            .examples()
            .iter()
            .filter_map(|e| e.get("kind")?.as_str().map(str::to_string))
            .collect();
        v_flex()
            .track_focus(&self.focus)
            .text_color(colors.text)
            .text_size(u(13.0))
            .child(header(
                Icon::new(IconName::FilePlus)
                    .size(16.0)
                    .color(colors.accent)
                    .into_any_element(),
                format!("New instance of {}", self.csv.display_name),
                Vec::new(),
                &colors,
            ))
            .child(
                v_flex()
                    .p(u(8.0))
                    .children(self.csv.owned.iter().enumerate().map(|(i, crd)| {
                        let cluster = self.cluster.clone();
                        let csv = self.csv.clone();
                        let crd_for_click = crd.clone();
                        let has_example = examples.contains(&crd.kind);
                        h_flex()
                            .id(("create-kind", i))
                            .gap(u(8.0))
                            .px(u(10.0))
                            .py(u(6.0))
                            .rounded(u(5.0))
                            .cursor_pointer()
                            .hover(|s| s.bg(colors.hover))
                            .child(
                                div()
                                    .flex_1()
                                    .font_family(fonts::MONO)
                                    .text_size(u(12.5))
                                    .child(crd.kind.clone()),
                            )
                            .child(
                                div()
                                    .text_size(u(11.5))
                                    .text_color(colors.text_dim)
                                    .child(if has_example { "example" } else { "template" }),
                            )
                            .on_click(move |_, window, cx| {
                                window.close_dialog(cx);
                                create_instance(&cluster, &csv, &crd_for_click, window, cx);
                            })
                    })),
            )
            .child(footer(None, vec![close_button("Cancel")], &colors))
    }
}

// ----- helm commands -----

/// Opens the list of `helm` commands for a release: Kubyl shows releases read-only, the user
/// runs rollback and uninstall with their `helm`.
pub fn open_helm_commands(
    title: String,
    commands: Vec<Command>,
    window: &mut Window,
    cx: &mut App,
) {
    let view = cx.new(|cx| HelmCommands {
        title,
        commands,
        focus: cx.focus_handle(),
    });
    let focus = view.read(cx).focus.clone();
    open(view, 620.0, Some(focus), window, cx);
}

struct HelmCommands {
    title: String,
    commands: Vec<Command>,
    focus: FocusHandle,
}

impl Focusable for HelmCommands {
    fn focus_handle(&self, _: &App) -> FocusHandle {
        self.focus.clone()
    }
}

impl Render for HelmCommands {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let colors = cx.colors().clone();
        v_flex()
            .track_focus(&self.focus)
            .text_color(colors.text)
            .text_size(u(13.0))
            .child(header(
                Icon::new(IconName::Anchor)
                    .size(16.0)
                    .color(colors.accent)
                    .into_any_element(),
                self.title.clone(),
                Vec::new(),
                &colors,
            ))
            .child(
                v_flex()
                    .p(u(8.0))
                    .gap(u(2.0))
                    .children(self.commands.iter().enumerate().map(|(i, command)| {
                        let text = command.command.clone();
                        let label = command.label.clone();
                        h_flex()
                            .id(("helm-command", i))
                            .items_start()
                            .gap(u(10.0))
                            .px(u(10.0))
                            .py(u(6.0))
                            .rounded(u(5.0))
                            .cursor_pointer()
                            .hover(|s| s.bg(colors.hover))
                            .child(
                                v_flex()
                                    .flex_1()
                                    .min_w_0()
                                    .child(div().text_size(u(12.5)).child(command.label.clone()))
                                    .child(
                                        div()
                                            .font_family(fonts::MONO)
                                            .text_size(u(11.0))
                                            .text_color(colors.text_dim)
                                            .child(command.command.clone()),
                                    ),
                            )
                            .child(Icon::new(IconName::Copy).size(13.0).color(colors.text_dim))
                            .on_click(move |_, window, cx| {
                                widgets::copy(
                                    text.clone(),
                                    &format!("the {} command", label.to_lowercase()),
                                    cx,
                                );
                                window.close_dialog(cx);
                            })
                    })),
            )
            .child(footer(
                Some(
                    div()
                        .text_size(u(11.5))
                        .text_color(colors.text_dim)
                        .child(
                            "Kubyl shows Helm releases read-only. Run the command in a terminal.",
                        )
                        .into_any_element(),
                ),
                vec![close_button("Close")],
                &colors,
            ))
    }
}
