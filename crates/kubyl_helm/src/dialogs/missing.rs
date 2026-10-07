//! "helm isn't installed" (board 20): what Kubyl looked for, how to install `helm`, and that the
//! releases stay visible (read-only) without it.

use gpui::{
    AnyElement, App, AppContext as _, Context, FocusHandle, Focusable, IntoElement, Render, Window,
    div, prelude::*,
};
use kubyl_helm_core::cli::{self, Probe};
use kubyl_ui::{ActiveColors, Button, Colors, Icon, IconName, fonts, h_flex, u, v_flex};

use super::{footer, header, open};
use crate::cli::{CliState, HelmCli};
use crate::widgets;

/// Opens the note.
pub fn open_missing(window: &mut Window, cx: &mut App) {
    let view = cx.new(|cx: &mut Context<MissingDialog>| MissingDialog {
        focus: cx.focus_handle(),
        // Re-renders when the probe answers; goes with the dialog.
        _helm: HelmCli::global(cx).map(|helm| cx.observe(&helm, |_, _, cx| cx.notify())),
    });
    let focus = view.read(cx).focus.clone();
    open(view, 620.0, Some(focus), window, cx);
}

pub struct MissingDialog {
    focus: FocusHandle,
    _helm: Option<gpui::Subscription>,
}

impl Focusable for MissingDialog {
    fn focus_handle(&self, _: &App) -> FocusHandle {
        self.focus.clone()
    }
}

/// The body of the "helm isn't installed" state (dialog and Charts tab).
pub fn body(cx: &App) -> AnyElement {
    let colors: Colors = cx.colors().clone();
    let helm = HelmCli::global(cx);
    let state = helm.as_ref().map(|h| h.read(cx).state().clone());
    let (title, detail) = match &state {
        None | Some(CliState::Probing) => (
            "Looking for helm…".to_string(),
            "Kubyl runs your helm (3.13 or newer) from your login shell's PATH.".to_string(),
        ),
        Some(CliState::Done(probe)) => match probe.as_ref() {
            Probe::Ready(info) => (
                format!("helm {} is ready", info.version),
                format!("{}", info.path.display()),
            ),
            Probe::TooOld { version, path } => (
                format!("helm {version} is too old"),
                format!(
                    "{} is older than {}: Kubyl needs --dry-run=server. Update helm.",
                    path.display(),
                    cli::MIN_VERSION
                ),
            ),
            other => (
                "helm isn't installed".to_string(),
                other.problem().unwrap_or_default(),
            ),
        },
    };
    let hint = cli::install_hint();
    v_flex()
        .gap(u(14.0))
        .p(u(18.0))
        .child(
            h_flex()
                .gap(u(12.0))
                .child(Icon::new(IconName::Anchor).size(22.0).color(colors.text_dim))
                .child(
                    v_flex()
                        .min_w_0()
                        .child(div().text_size(u(15.0)).font_weight(gpui::FontWeight::SEMIBOLD).child(title))
                        .child(
                            div()
                                .text_size(u(12.5))
                                .text_color(colors.text_muted)
                                .child(detail),
                        ),
                ),
        )
        .child(
            v_flex()
                .gap(u(8.0))
                .p(u(12.0))
                .rounded(u(6.0))
                .border_1()
                .border_color(colors.border)
                .text_size(u(12.5))
                .text_color(colors.text_muted)
                .child(
                    h_flex()
                        .gap(u(8.0))
                        .child(
                            div()
                                .flex_1()
                                .px(u(8.0))
                                .py(u(6.0))
                                .rounded(u(5.0))
                                .bg(colors.background)
                                .font_family(fonts::MONO)
                                .text_size(u(11.5))
                                .text_color(colors.text)
                                .child(hint),
                        )
                        .child(
                            kubyl_ui::IconButton::new("helm-copy-install", IconName::Copy)
                                .icon_size(13.0)
                                .on_click(move |_, _, cx| {
                                    widgets::copy(hint.to_string(), "the install command", cx)
                                }),
                        ),
                )
                .child(
                    "Or set helm.path in settings.json. Helm releases stay visible without it (read-only), and their commands can be copied.",
                ),
        )
        .child(
            h_flex()
                .gap(u(8.0))
                .child(
                    Button::new("helm-check-again")
                        .icon(IconName::RefreshCw)
                        .label("Check again")
                        .on_click(|_, _, cx| {
                            if let Some(helm) = HelmCli::global(cx) {
                                helm.update(cx, |helm, cx| helm.reprobe(cx));
                            }
                        }),
                )
                .child(
                    Button::new("helm-install-docs")
                        .ghost()
                        .icon(IconName::ExternalLink)
                        .label("helm.sh/docs/intro/install")
                        .on_click(|_, _, cx| {
                            widgets::open_url("https://helm.sh/docs/intro/install/", cx)
                        }),
                ),
        )
        .into_any_element()
}

impl Render for MissingDialog {
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
                "helm is needed for this",
                Vec::new(),
                &colors,
            ))
            .child(body(cx))
            .child(footer(None, vec![super::close_button("Close")], &colors))
    }
}
