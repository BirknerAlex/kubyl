//! What the view shows instead of flows (board 18): looking for a source, connecting, no source
//! (with what to install for the cluster's CNI and what was checked), a failure or a 403 that
//! names the missing permission, flows turned off.

use gpui::{AnyElement, App, Context, FontWeight, IntoElement, SharedString, div, prelude::*};
use kubyl_ui::{Colors, Icon, IconName, fonts, h_flex, u, v_flex};

use super::{NetworkFlowsView, widgets};
use crate::detect::{Cni, Detection};
use crate::provider::{BackendKind, ProviderError};
use crate::service::{FlowService, FlowState};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Blocking {
    NotConnected,
    Detecting,
    Connecting,
    NoBackend,
    Failed,
    Off,
}

pub(super) fn blocking(_: &NetworkFlowsView, state: &FlowState, _: &App) -> Option<Blocking> {
    match state {
        FlowState::NotConnected => Some(Blocking::NotConnected),
        FlowState::Detecting => Some(Blocking::Detecting),
        FlowState::Connecting { .. } => Some(Blocking::Connecting),
        FlowState::NoBackend(_) => Some(Blocking::NoBackend),
        FlowState::Failed { .. } => Some(Blocking::Failed),
        FlowState::Off => Some(Blocking::Off),
        FlowState::Ready { .. } => None,
    }
}

fn title_block(
    icon: IconName,
    color: gpui::Hsla,
    title: impl Into<SharedString>,
    subtitle: impl Into<SharedString>,
    colors: &Colors,
) -> AnyElement {
    h_flex()
        .gap(u(12.0))
        .child(Icon::new(icon).size(22.0).color(color))
        .child(
            v_flex()
                .gap(u(2.0))
                .child(
                    div()
                        .text_size(u(16.0))
                        .font_weight(FontWeight::SEMIBOLD)
                        .child(title.into()),
                )
                .child(
                    div()
                        .text_size(u(12.5))
                        .text_color(colors.text_muted)
                        .child(subtitle.into()),
                ),
        )
        .into_any_element()
}

fn page(children: Vec<AnyElement>) -> AnyElement {
    v_flex()
        .id("flows-state")
        .flex_1()
        .size_full()
        .overflow_y_scroll()
        .gap(u(14.0))
        .px(u(30.0))
        .py(u(26.0))
        .children(children)
        .into_any_element()
}

pub(super) fn render(
    view: &NetworkFlowsView,
    blocking: Blocking,
    state: &FlowState,
    colors: &Colors,
    cx: &mut Context<NetworkFlowsView>,
) -> AnyElement {
    match (blocking, state) {
        (Blocking::NotConnected, _) => widgets::empty("The cluster isn't connected.", colors),
        (Blocking::Detecting, _) => page(vec![title_block(
            IconName::Waypoints,
            colors.text_dim,
            "Looking for a flow source…",
            "Kubyl checks for Hubble Relay, Calico Whisker and NetObserv.",
            colors,
        )]),
        (Blocking::Connecting, FlowState::Connecting { kind, detection }) => {
            let how = match (kind, detection.find(*kind)) {
                (BackendKind::Hubble, Some(candidate)) => format!(
                    "Relay speaks gRPC: Kubyl opens a temporary port-forward to {} on this machine (it shows in Active Sessions).",
                    candidate.label()
                ),
                (_, Some(candidate)) => format!(
                    "Through the API server's service proxy ({}).",
                    candidate.label()
                ),
                _ => String::new(),
            };
            page(vec![title_block(
                IconName::Waypoints,
                colors.accent,
                format!("Connecting to {}…", kind.label()),
                how,
                colors,
            )])
        }
        (Blocking::Off, _) => page(vec![title_block(
            IconName::Waypoints,
            colors.text_dim,
            "Network flows are off for this cluster",
            "netflow.clusters.<cluster>.backend is off in settings.json.",
            colors,
        )]),
        (Blocking::NoBackend, FlowState::NoBackend(detection)) => {
            no_backend(view, detection, colors, cx)
        }
        (
            Blocking::Failed,
            FlowState::Failed {
                kind,
                error,
                detection,
            },
        ) => failed(view, *kind, error, detection, colors, cx),
        _ => widgets::empty("", colors),
    }
}

/// An install hint: title, text, docs link.
struct Hint {
    title: &'static str,
    text: String,
    link: (&'static str, &'static str),
}

fn hints(cni: Option<&Cni>) -> Vec<Hint> {
    let label = cni.map(Cni::label);
    let netobserv = Hint {
        title: "NetObserv",
        text: format!(
            "Works with any CNI{}: an eBPF agent on each node. Single flows need its Loki; the topology works from its metrics.",
            label.map(|l| format!(", {l} included")).unwrap_or_default()
        ),
        link: if cni == Some(&Cni::OvnKubernetes) {
            (
                "Network Observability on OpenShift",
                "https://docs.redhat.com/en/documentation/openshift_container_platform/latest/html/network_observability/installing-network-observability-operators",
            )
        } else {
            (
                "Install NetObserv",
                "https://github.com/netobserv/network-observability-operator#getting-started",
            )
        },
    };
    let cilium = Hint {
        title: "Cilium with Hubble",
        text: "Replaces the CNI. Hubble Relay names the policy behind each verdict and sees HTTP and DNS.".into(),
        link: ("Install Cilium", "https://docs.cilium.io/en/stable/gettingstarted/k8s-install-default/"),
    };
    let calico = Hint {
        title: "Calico 3.30+ with Whisker",
        text: "Replaces the CNI. Whisker's flow log shows the policy trace of each verdict.".into(),
        link: (
            "Calico Whisker",
            "https://docs.tigera.io/calico/latest/observability/view-flow-logs",
        ),
    };
    match cni {
        Some(Cni::Cilium) => vec![
            Hint {
                title: "Turn on Hubble Relay",
                text: "Cilium runs here without Hubble Relay. With it (Helm: hubble.relay.enabled=true), Kubyl reads flows through a temporary port-forward.".into(),
                link: ("Set up Hubble", "https://docs.cilium.io/en/stable/observability/hubble/setup/"),
            },
            netobserv,
        ],
        Some(Cni::Calico) => vec![
            Hint {
                title: "Turn on Whisker",
                text: "Calico runs here without Whisker. Calico 3.30 or newer ships Whisker and Goldmane (the operator's Whisker and Goldmane resources).".into(),
                link: ("Calico Whisker", "https://docs.tigera.io/calico/latest/observability/view-flow-logs"),
            },
            netobserv,
        ],
        _ => vec![netobserv, cilium, calico],
    }
}

fn checks(detection: &Detection, colors: &Colors) -> AnyElement {
    v_flex()
        .px(u(14.0))
        .pt(u(6.0))
        .pb(u(4.0))
        .rounded(u(8.0))
        .border_1()
        .border_color(colors.border)
        .bg(colors.panel)
        .child(
            div()
                .py(u(6.0))
                .text_size(u(11.0))
                .font_weight(FontWeight::SEMIBOLD)
                .text_color(colors.text_dim)
                .child("WHAT WAS CHECKED"),
        )
        .children(detection.checks.iter().enumerate().map(|(i, check)| {
            let (icon, color) = if check.forbidden.is_some() {
                (IconName::CircleX, colors.red)
            } else if check.found {
                (IconName::CircleCheck, colors.green)
            } else {
                (IconName::Minus, colors.text_dim)
            };
            h_flex()
                .items_start()
                .gap(u(10.0))
                .py(u(7.0))
                .when(i + 1 < detection.checks.len(), |this| {
                    this.border_b_1().border_color(colors.border_variant)
                })
                .text_size(u(12.5))
                .child(Icon::new(icon).size(13.0).color(color))
                .child(
                    v_flex()
                        .flex_1()
                        .min_w_0()
                        .child(check.backend.label())
                        .child(
                            div()
                                .text_size(u(12.0))
                                .text_color(colors.text_dim)
                                .child(check.text.clone()),
                        ),
                )
        }))
        .into_any_element()
}

fn button(
    id: &'static str,
    icon: IconName,
    label: &'static str,
    primary: bool,
    cluster: kubyl_core::ClusterId,
    action: fn(&mut FlowService, &kubyl_core::ClusterId, &mut Context<FlowService>),
    colors: &Colors,
) -> AnyElement {
    widgets::button(id, Some(icon), label, primary, colors, move |_, _, cx| {
        if let Some(service) = FlowService::global(cx) {
            let cluster = cluster.clone();
            service.update(cx, |s, cx| action(s, &cluster, cx));
        }
    })
    .into_any_element()
}

fn no_backend(
    view: &NetworkFlowsView,
    detection: &Detection,
    colors: &Colors,
    cx: &mut Context<NetworkFlowsView>,
) -> AnyElement {
    let _ = cx;
    let subtitle = match &detection.cni {
        Some(cni) => format!(
            "It runs {}, which Kubyl reads no flows from. Kubyl reads them from one of these:",
            cni.label()
        ),
        None => "Kubyl reads flows from one of these:".to_string(),
    };
    let cards = h_flex().items_stretch().gap(u(10.0)).children(
        hints(detection.cni.as_ref())
            .into_iter()
            .enumerate()
            .map(|(i, hint)| {
                let url = hint.link.1;
                v_flex()
                    .flex_1()
                    .min_w_0()
                    .gap(u(5.0))
                    .p(u(12.0))
                    .rounded(u(8.0))
                    .border_1()
                    .border_color(colors.border)
                    .bg(colors.panel)
                    .child(
                        div()
                            .font_weight(FontWeight::MEDIUM)
                            .text_size(u(12.5))
                            .child(hint.title),
                    )
                    .child(
                        div()
                            .flex_1()
                            .text_size(u(12.0))
                            .text_color(colors.text_muted)
                            .child(hint.text),
                    )
                    .child(
                        h_flex()
                            .gap(u(5.0))
                            .child(
                                Icon::new(IconName::ExternalLink)
                                    .size(11.0)
                                    .color(colors.accent),
                            )
                            .child(
                                widgets::link(
                                    ("flows-hint", i),
                                    hint.link.0,
                                    colors,
                                    move |_, _, cx| cx.open_url(url),
                                )
                                .text_size(u(12.0)),
                            ),
                    )
            }),
    );
    let snippet = format!(
        "\"netflow\": {{ \"clusters\": {{ \"{}\": {{ \"backend\": \"hubble\",\n  \"hubble\": {{ \"namespace\": \"cilium\", \"service\": \"relay\" }} }} }} }}",
        settings_name(view, cx)
    );
    page(vec![
        title_block(
            IconName::Waypoints,
            colors.text_dim,
            "No network flows for this cluster",
            subtitle,
            colors,
        ),
        cards.into_any_element(),
        checks(detection, colors),
        div()
            .text_size(u(12.5))
            .text_color(colors.text_muted)
            .child("Somewhere else? Name it in settings.json:")
            .into_any_element(),
        div()
            .px(u(10.0))
            .py(u(8.0))
            .rounded(u(6.0))
            .bg(colors.input_background)
            .font_family(fonts::MONO)
            .text_size(u(11.5))
            .text_color(colors.text_muted)
            .child(snippet)
            .into_any_element(),
        h_flex()
            .gap(u(8.0))
            .child(button(
                "flows-look-again",
                IconName::RefreshCw,
                "Look again",
                false,
                view.cluster.clone(),
                FlowService::redetect,
                colors,
            ))
            .into_any_element(),
    ])
}

/// The key a settings snippet uses for this cluster (its context name).
fn settings_name(view: &NetworkFlowsView, cx: &App) -> String {
    kubyl_kube::ConnectionManager::try_global(cx)
        .and_then(|m| m.read(cx).settings_keys(&view.cluster).last().cloned())
        .unwrap_or_else(|| view.cluster.to_string())
}

fn failed(
    view: &NetworkFlowsView,
    kind: BackendKind,
    error: &ProviderError,
    detection: &Detection,
    colors: &Colors,
    cx: &mut Context<NetworkFlowsView>,
) -> AnyElement {
    let _ = cx;
    let forbidden = matches!(error, ProviderError::Forbidden { .. });
    let subtitle = match kind {
        BackendKind::Hubble => {
            "Relay speaks gRPC, which goes through a temporary port-forward on this machine."
        }
        BackendKind::Whisker => "Whisker is read through the API server's service proxy.",
        BackendKind::NetObserv => {
            "NetObserv's Loki and metrics are read through the API server's service proxy."
        }
    };
    let message: AnyElement = match error {
        ProviderError::Forbidden { .. } => v_flex()
            .gap(u(3.0))
            .child(format!("Forbidden: you can't {}.", error.permission().unwrap_or_default()))
            .child(div().text_color(colors.text_muted).child(format!(
                "Ask for a role that can {}. Nothing else is needed: Kubyl never reads Secrets or exposes a port.",
                error.permission().unwrap_or_default()
            )))
            .into_any_element(),
        other => div().child(other.to_string()).into_any_element(),
    };
    let others: Vec<BackendKind> = detection
        .candidates
        .iter()
        .map(|c| c.kind())
        .filter(|k| *k != kind)
        .collect();
    let switch = (!others.is_empty()).then(|| {
        let cluster = view.cluster.clone();
        let other = others[0];
        h_flex()
            .gap(u(4.0))
            .text_size(u(12.0))
            .text_color(colors.text_dim)
            .child(format!(
                "Also found: {} ·",
                others
                    .iter()
                    .map(|k| k.label())
                    .collect::<Vec<_>>()
                    .join(", ")
            ))
            .child(widgets::link(
                "flows-use-other",
                format!("Use {} instead", other.label()),
                colors,
                move |_, _, cx| {
                    FlowService::use_backend(&cluster, other, cx);
                },
            ))
            .into_any_element()
    });
    let mut children = vec![
        title_block(
            if forbidden {
                IconName::Lock
            } else {
                IconName::TriangleAlert
            },
            colors.red,
            format!("Kubyl can't reach {}", kind.label()),
            subtitle,
            colors,
        ),
        h_flex()
            .items_start()
            .gap(u(10.0))
            .p(u(12.0))
            .rounded(u(8.0))
            .border_1()
            .border_color(colors.border)
            .bg(colors.panel)
            .text_size(u(12.5))
            .child(Icon::new(IconName::CircleX).size(14.0).color(colors.red))
            .child(div().flex_1().min_w_0().child(message))
            .into_any_element(),
    ];
    children.extend(switch);
    children.push(
        h_flex()
            .gap(u(8.0))
            .child(button(
                "flows-try-again",
                IconName::RefreshCw,
                "Try again",
                true,
                view.cluster.clone(),
                FlowService::reconnect,
                colors,
            ))
            .child(button(
                "flows-look-again",
                IconName::Search,
                "Look again",
                false,
                view.cluster.clone(),
                FlowService::redetect,
                colors,
            ))
            .into_any_element(),
    );
    page(children)
}
