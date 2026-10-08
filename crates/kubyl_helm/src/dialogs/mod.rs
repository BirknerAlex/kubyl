//! Helm dialogs (board 20): install, upgrade, rollback, uninstall, repositories, the `helm`
//! commands to copy, and the "helm isn't installed" note. Shared pieces live here.
//!
//! Every write shows what changes first (Helm's dry run, or the stored releases), is hidden on
//! read-only clusters and asks for a typed name on production clusters.

pub mod commands;
pub mod install;
pub mod missing;
pub mod pick;
pub mod preview;
pub mod repos;
pub mod rollback;
pub mod uninstall;
pub mod upgrade;

use gpui::{
    AnyElement, App, AppContext as _, Entity, FocusHandle, FontWeight, IntoElement, Render,
    SharedString, Window, div, prelude::*, px,
};
use gpui_component::WindowExt as _;
use gpui_component::input::{Input, InputState};
use kubyl_core::{ClusterId, Notification, NotificationCenter};
use kubyl_kube::ConnectionManager;
use kubyl_ui::{ActiveColors, Button, Colors, Icon, IconName, fonts, h_flex, u, v_flex};

pub use commands::open_helm_commands;
pub use install::{InstallRequest, open_install};
pub use missing::open_missing;
pub use repos::open_repositories;
pub use rollback::open_rollback;
pub use uninstall::open_uninstall;
pub use upgrade::open_upgrade;

pub(crate) fn open<V: Render>(
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

pub(crate) fn header(
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

pub(crate) fn footer(
    note: Option<AnyElement>,
    buttons: Vec<AnyElement>,
    colors: &Colors,
) -> impl IntoElement {
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

pub(crate) fn close_button(label: &'static str) -> AnyElement {
    Button::new("helm-dialog-close")
        .ghost()
        .label(label)
        .on_click(|_, window, cx| window.close_dialog(cx))
        .into_any_element()
}

pub(crate) fn error_line(error: &Option<String>, colors: &Colors) -> Option<AnyElement> {
    error.as_ref().map(|e| {
        div()
            .text_size(u(12.0))
            .text_color(colors.red)
            .child(e.clone())
            .into_any_element()
    })
}

pub(crate) fn input_box(
    input: &Entity<InputState>,
    mono: bool,
    colors: &Colors,
) -> impl IntoElement {
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
pub(crate) fn form_row(
    label: &'static str,
    content: impl IntoElement,
    colors: &Colors,
) -> impl IntoElement {
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

pub(crate) fn checkbox(
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

pub(crate) fn typed_row(what: &str, input: &Entity<InputState>, colors: &Colors) -> AnyElement {
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

pub(crate) fn production(cluster: &ClusterId, cx: &App) -> bool {
    ConnectionManager::try_global(cx).is_some_and(|m| m.read(cx).caps(cluster).production)
}

pub(crate) use crate::cli::read_only;

/// Why a dialog's preview no longer holds: another revision of the release landed since it
/// opened (an upgrade elsewhere), so what it shows isn't what would change. `None` while it
/// holds, or when Kubyl doesn't watch the release.
pub(crate) fn release_changed(
    cluster: &ClusterId,
    row: &crate::service::ReleaseRow,
    cx: &App,
) -> Option<String> {
    let snapshot = crate::releases::snapshot(cluster, cx)?;
    let now = snapshot
        .releases
        .iter()
        .find(|r| r.namespace == row.namespace && r.name == row.name && r.driver == row.driver)?
        .latest()
        .revision;
    (now != row.latest().revision).then(|| {
        format!(
            "The release changed since this opened (now revision {now}): close this and open it again to see what would change."
        )
    })
}

pub(crate) fn cluster_name(cluster: &ClusterId, cx: &App) -> String {
    ConnectionManager::try_global(cx)
        .map(|m| m.read(cx).display_name(cluster).to_string())
        .unwrap_or_else(|| cluster.to_string())
}

pub(crate) fn client(cluster: &ClusterId, cx: &App) -> Option<kube::Client> {
    ConnectionManager::try_global(cx)?.read(cx).client(cluster)
}

/// Refuses writes on read-only clusters (the buttons are hidden; keys may still ask).
pub(crate) fn guard(cluster: &ClusterId, cx: &mut App) -> bool {
    if read_only(cluster, cx) {
        NotificationCenter::push(
            cx,
            Notification::error("This cluster is read-only in Kubyl."),
        );
        return false;
    }
    true
}

pub(crate) fn typed_input(
    placeholder: String,
    window: &mut Window,
    cx: &mut App,
) -> Entity<InputState> {
    cx.new(|cx| InputState::new(window, cx).placeholder(placeholder))
}

/// A chart version as the version menus show it: `0.3.0 (latest)` for the one `helm install`
/// picks without `--version`, `0.4.0-rc.1 (pre-release)`. `versions`: newest first.
pub(crate) fn version_label(versions: &[(String, Option<String>)], version: &str) -> String {
    let latest = kubyl_helm_core::repo::latest_stable(versions.iter().map(|(v, _)| v.as_str()));
    if latest == Some(version) {
        format!("{version} (latest)")
    } else if kubyl_helm_core::repo::is_prerelease(version) {
        format!("{version} (pre-release)")
    } else {
        version.to_string()
    }
}

/// The steps in a dialog's header (`1 Configure › 2 Preview › 3 Install`).
pub(crate) fn steps(names: &[&'static str], current: usize, colors: &Colors) -> AnyElement {
    let mut row = h_flex().gap(u(10.0));
    for (i, name) in names.iter().enumerate() {
        if i > 0 {
            row = row.child(div().text_color(colors.text_faint).child("›"));
        }
        let color = if i == current {
            colors.accent
        } else if i < current {
            colors.green
        } else {
            colors.text_dim
        };
        row = row.child(
            h_flex()
                .gap(u(6.0))
                .text_size(u(12.0))
                .text_color(color)
                .child(
                    div()
                        .size(u(16.0))
                        .rounded_full()
                        .border_1()
                        .border_color(color)
                        .flex()
                        .items_center()
                        .justify_center()
                        .text_size(u(10.0))
                        .child((i + 1).to_string()),
                )
                .child(*name),
        );
    }
    row.into_any_element()
}

/// A running or finished operation: its title, Helm's output and Cancel while it runs.
pub(crate) fn operation(id: u64, cx: &App) -> AnyElement {
    let colors = cx.colors().clone();
    let Some(ops) = crate::ops::HelmOps::global(cx) else {
        return div().into_any_element();
    };
    let ops = ops.read(cx);
    let Some(op) = ops.get(id) else {
        return div().into_any_element();
    };
    let elapsed = op
        .finished
        .unwrap_or_else(std::time::Instant::now)
        .duration_since(op.started)
        .as_secs();
    let (icon, color, status) = match &op.state {
        crate::ops::OpState::Running => (
            IconName::RefreshCw,
            colors.accent,
            format!("{} · {}", op.title(), crate::widgets::elapsed(elapsed)),
        ),
        crate::ops::OpState::Done { revision } => (
            IconName::CircleCheck,
            colors.green,
            match revision {
                Some(r) => format!(
                    "{} {} in {} · revision {r} · {}",
                    op.kind.done(),
                    op.release,
                    op.namespace,
                    crate::widgets::elapsed(elapsed)
                ),
                None => format!(
                    "{} {} in {} · {}",
                    op.kind.done(),
                    op.release,
                    op.namespace,
                    crate::widgets::elapsed(elapsed)
                ),
            },
        ),
        crate::ops::OpState::Failed(err) => (
            IconName::CircleX,
            colors.red,
            format!("{} failed: {}", op.title(), err.message),
        ),
    };
    let running = op.is_running();
    let detail = match &op.state {
        crate::ops::OpState::Failed(err) if !err.detail.is_empty() => Some(err.detail.clone()),
        _ => None,
    };
    let lines: Vec<String> = op.lines.iter().rev().take(14).rev().cloned().collect();
    let stuck_hint = matches!(&op.state, crate::ops::OpState::Failed(err)
        if matches!(err.kind, kubyl_helm_core::cli::ErrorKind::InProgress | kubyl_helm_core::cli::ErrorKind::Cancelled | kubyl_helm_core::cli::ErrorKind::Timeout));
    let release = (op.cluster.clone(), op.namespace.clone(), op.release.clone());
    v_flex()
        .gap(u(8.0))
        .p(u(16.0))
        .child(
            h_flex()
                .items_start()
                .gap(u(8.0))
                .child(Icon::new(icon).size(14.0).color(color))
                .child(
                    div()
                        .flex_1()
                        .min_w_0()
                        .text_size(u(12.5))
                        .font_weight(FontWeight::MEDIUM)
                        .child(status),
                )
                .when(running, |this| {
                    this.child(
                        Button::new("helm-op-cancel")
                            .ghost()
                            .label("Cancel")
                            .on_click(move |_, _, cx| {
                                if let Some(ops) = crate::ops::HelmOps::global(cx) {
                                    ops.update(cx, |ops, cx| ops.cancel(id, cx));
                                }
                            }),
                    )
                }),
        )
        .child(
            div()
                .id("helm-op-output")
                .h(u(170.0))
                .p(u(8.0))
                .rounded(u(5.0))
                .bg(colors.background)
                .overflow_y_scroll()
                .font_family(fonts::MONO)
                .text_size(u(11.5))
                .text_color(colors.text_muted)
                .children(if lines.is_empty() && detail.is_none() {
                    vec![div()
                        .text_color(colors.text_dim)
                        .child(if running {
                            "helm is running…"
                        } else {
                            "helm printed nothing."
                        })
                        .into_any_element()]
                } else {
                    lines
                        .into_iter()
                        .chain(detail.into_iter().filter(|_| op.lines.is_empty()))
                        .map(|l| div().child(l).into_any_element())
                        .collect()
                }),
        )
        .when(stuck_hint, |this| {
            this.child(
                h_flex()
                    .items_start()
                    .gap(u(8.0))
                    .child(div().flex_1().min_w_0().child(crate::widgets::note(
                        IconName::Info,
                        colors.accent,
                        "A release left pending-install, pending-upgrade or pending-rollback blocks new operations. Roll it back to its last deployed revision (or uninstall a release that never deployed).",
                        &colors,
                    )))
                    .child(
                        Button::new("helm-op-show-release")
                            .ghost()
                            .icon(IconName::Eye)
                            .label("Show the release's status")
                            .on_click(move |_, window, cx| {
                                let (cluster, namespace, name) = &release;
                                show_release(cluster, namespace, name, window, cx);
                            }),
                    ),
            )
        })
        .into_any_element()
}

/// Opens a release's tab (its status, history and the stuck-release banner) from a dialog.
fn show_release(
    cluster: &ClusterId,
    namespace: &str,
    name: &str,
    window: &mut Window,
    cx: &mut App,
) {
    let row = crate::releases::snapshot(cluster, cx).and_then(|snapshot| {
        snapshot
            .releases
            .iter()
            .find(|r| r.namespace == namespace && r.name == name)
            .cloned()
    });
    match row {
        Some(row) => {
            window.close_dialog(cx);
            crate::release::open(cluster, &row, None, window, cx);
        }
        None => NotificationCenter::push(
            cx,
            Notification::error(format!(
                "Kubyl doesn't list {name} in {namespace}: open the cluster's Helm releases."
            )),
        ),
    }
}
