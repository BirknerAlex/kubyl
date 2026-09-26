//! The states that explain an empty Alerts tab: not connected, loading, off, no Alertmanager
//! found (with what was tried), and reads that fail.

use gpui::{AnyElement, App, FontWeight, IntoElement, div, prelude::*};
use jiff::Timestamp;
use kubyl_core::ClusterId;
use kubyl_kube::ConnectionManager;
use kubyl_ui::{ActiveColors, Button, Icon, IconName, fonts, h_flex, u, v_flex};

use super::widgets;
use crate::service::{AlertsService, ClusterAlerts};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Blocking {
    NotConnected,
    Loading,
    Disabled,
    NoSource,
    /// Sources were found, but no read worked yet.
    Failed,
}

fn frame(icon: IconName, title: String, detail: String, cx: &App) -> gpui::Div {
    let colors = cx.colors().clone();
    v_flex()
        .flex_1()
        .p(u(32.0))
        .gap(u(14.0))
        .max_w(u(560.0))
        .child(
            h_flex()
                .items_start()
                .gap(u(12.0))
                .child(Icon::new(icon).size(20.0).color(colors.text_dim))
                .child(
                    v_flex()
                        .gap(u(3.0))
                        .child(
                            div()
                                .text_size(u(16.0))
                                .font_weight(FontWeight::SEMIBOLD)
                                .child(title),
                        )
                        .child(
                            div()
                                .text_size(u(12.5))
                                .text_color(colors.text_muted)
                                .child(detail),
                        ),
                ),
        )
}

fn look_again(cluster: &ClusterId) -> impl IntoElement {
    let cluster = cluster.clone();
    Button::new("alerts-look-again")
        .icon(IconName::RefreshCw)
        .label("Look again")
        .on_click(move |_, _, cx| {
            if let Some(service) = AlertsService::global(cx) {
                let cluster = cluster.clone();
                service.update(cx, |s, cx| s.redetect(&cluster, cx));
            }
        })
}

pub fn render(
    blocking: Blocking,
    cluster: &ClusterId,
    state: Option<&ClusterAlerts>,
    cx: &App,
) -> AnyElement {
    let colors = cx.colors().clone();
    let now = Timestamp::now();
    match blocking {
        Blocking::NotConnected => {
            let connect = cluster.clone();
            frame(
                IconName::Cloud,
                "Not connected".into(),
                "Alerts are read while the cluster is connected.".into(),
                cx,
            )
            .child(
                h_flex().child(
                    Button::new("alerts-connect")
                        .primary()
                        .label("Connect")
                        .on_click(move |_, _, cx| {
                            if let Some(manager) = ConnectionManager::try_global(cx) {
                                let id = connect.clone();
                                manager.update(cx, |m, cx| m.connect(&id, cx));
                            }
                        }),
                ),
            )
            .into_any_element()
        }
        Blocking::Loading => v_flex()
            .flex_1()
            .items_center()
            .justify_center()
            .gap(u(8.0))
            .text_color(colors.text_dim)
            .child(Icon::new(IconName::Siren).size(22.0).color(colors.text_faint))
            .child("Looking for Alertmanager and Prometheus…")
            .into_any_element(),
        Blocking::Disabled => frame(
            IconName::BellOff,
            "Alerts are off".into(),
            "alerts.enabled is false, or this cluster has \"disabled\": true under alerts.clusters in settings.json.".into(),
            cx,
        )
        .into_any_element(),
        Blocking::Failed => {
            let errors: Vec<String> = state
                .map(|s| {
                    s.sources
                        .iter()
                        .filter_map(|src| src.error.as_ref().map(|e| format!("{}: {e}", src.label())))
                        .collect()
                })
                .unwrap_or_default();
            let sign_in = errors.iter().any(|e| e.contains("401") || e.contains("Unauthorized"));
            let error = state
                .and_then(|s| s.error.clone())
                .map(|e| e.to_string())
                .unwrap_or_default();
            let sign_in_cluster = cluster.clone();
            frame(
                IconName::TriangleAlert,
                if sign_in {
                    "Sign in again".into()
                } else {
                    "Alertmanager doesn't answer".into()
                },
                if sign_in {
                    "The token was refused. Signing in again gets a new one.".into()
                } else {
                    "Kubyl found it, but reading alerts failed.".into()
                },
                cx,
            )
            .child(
                v_flex()
                    .gap(u(4.0))
                    .p(u(12.0))
                    .rounded(u(7.0))
                    .border_1()
                    .border_color(colors.border)
                    .font_family(fonts::MONO)
                    .text_size(u(11.5))
                    .text_color(colors.red)
                    .children(if errors.is_empty() {
                        vec![error]
                    } else {
                        errors
                    }),
            )
            .child(
                h_flex()
                    .gap(u(8.0))
                    .when(sign_in, |this| {
                        this.child(
                            Button::new("alerts-sign-in")
                                .primary()
                                .label("Sign in again")
                                .on_click(move |_, _, cx| {
                                    if let Some(manager) = ConnectionManager::try_global(cx) {
                                        let id = sign_in_cluster.clone();
                                        manager.update(cx, |m, cx| m.connect_interactive(&id, cx));
                                    }
                                }),
                        )
                    })
                    .child(look_again(cluster)),
            )
            .into_any_element()
        }
        Blocking::NoSource => {
            let (tried, notes, looked) = state
                .map(|s| (s.tried.clone(), s.notes.clone(), s.discovered_at))
                .unwrap_or_default();
            let when = looked
                .map(|t| {
                    format!(
                        "Kubyl looked {} ago and looks again every 5 min.",
                        widgets::ago(Some(t), now)
                    )
                })
                .unwrap_or_default();
            let mut list = v_flex()
                .p(u(14.0))
                .gap(u(10.0))
                .rounded(u(7.0))
                .border_1()
                .border_color(colors.border)
                .child(
                    div()
                        .text_size(u(11.0))
                        .font_weight(FontWeight::SEMIBOLD)
                        .text_color(colors.text_dim)
                        .child("WHAT WAS TRIED"),
                );
            for note in &notes {
                let (step, detail) = note.split_once(": ").unwrap_or((note.as_str(), ""));
                list = list.child(
                    h_flex()
                        .items_start()
                        .gap(u(10.0))
                        .child(Icon::new(IconName::Minus).size(12.0).color(colors.text_dim))
                        .child(
                            v_flex()
                                .child(div().text_size(u(12.5)).child(step.to_string()))
                                .child(
                                    div()
                                        .text_size(u(11.5))
                                        .text_color(colors.text_dim)
                                        .child(detail.to_string()),
                                ),
                        ),
                );
            }
            for attempt in &tried {
                let failed = attempt.error.is_some();
                list = list.child(
                    h_flex()
                        .items_start()
                        .gap(u(10.0))
                        .child(
                            Icon::new(if failed { IconName::CircleX } else { IconName::CircleCheck })
                                .size(12.0)
                                .color(if failed { colors.red } else { colors.green }),
                        )
                        .child(
                            v_flex()
                                .flex_1()
                                .min_w_0()
                                .child(div().text_size(u(12.5)).child(attempt.found_by.to_string()))
                                .child(
                                    div()
                                        .whitespace_normal()
                                        .font_family(fonts::MONO)
                                        .text_size(u(11.5))
                                        .text_color(colors.text_dim)
                                        .child(format!(
                                            "{}{}",
                                            attempt.label,
                                            attempt
                                                .error
                                                .as_ref()
                                                .map(|e| format!(" · {e}"))
                                                .unwrap_or_default()
                                        )),
                                ),
                        ),
                );
            }
            if let Some(best) = tried.iter().find(|t| t.error.is_some()) {
                list = list.child(
                    div()
                        .text_size(u(11.5))
                        .text_color(colors.text_muted)
                        .child(format!(
                            "Best candidate: {} · {}",
                            best.label,
                            best.error.clone().unwrap_or_default()
                        )),
                );
            }
            let key = ConnectionManager::try_global(cx)
                .map(|m| m.read(cx).display_name(cluster).to_string())
                .unwrap_or_else(|| cluster.to_string());
            let snippet = format!(
                "\"alerts\": {{ \"clusters\": {{ \"{key}\": {{\n  \"alertmanagers\": [{{ \"url\": \"https://alertmanager.example.com\" }}] }} }}\n}}"
            );
            let settings_cluster = cluster.clone();
            frame(IconName::BellOff, "No Alertmanager found".into(), when, cx)
                .child(list)
                .child(
                    div()
                        .text_size(u(12.5))
                        .child("Alertmanager somewhere else? Name it in settings.json:"),
                )
                .child(
                    div()
                        .p(u(12.0))
                        .rounded(u(6.0))
                        .bg(colors.panel)
                        .font_family(fonts::MONO)
                        .text_size(u(11.5))
                        .text_color(colors.text_muted)
                        .whitespace_normal()
                        .child(snippet),
                )
                .child(
                    h_flex()
                        .gap(u(8.0))
                        .child(
                            Button::new("alerts-set-am")
                                .primary()
                                .icon(IconName::Settings)
                                .label("Set Alertmanager…")
                                .on_click(move |_, window, cx| {
                                    crate::actions::open_settings(&settings_cluster, window, cx)
                                }),
                        )
                        .child(look_again(cluster)),
                )
                .into_any_element()
        }
    }
}
