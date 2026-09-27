//! The flow details (board 18): verdict and event, the policy behind it, both endpoints (links
//! to their pods), the sanitized L7 data and the backend's own fields, for debugging and trust.

use gpui::{
    AnyElement, App, ClipboardItem, Context, FontWeight, IntoElement, SharedString, Window, div,
    prelude::*,
};
use kubyl_core::actions::OpenView;
use kubyl_core::{Gvr, Notification, NotificationCenter, ResourceRef, ViewKind, ViewRequest};
use kubyl_ui::{ActiveColors, Chip, Colors, Icon, IconName, fonts, h_flex, u, v_flex};

use super::{NetworkFlowsView, SelectedPart, widgets};
use crate::model::{Direction, Endpoint, EndpointKind, Flow, L7, PolicyRef, PolicySummary};
use crate::provider::BackendKind;
use crate::service::FlowState;

pub(super) fn render(
    view: &NetworkFlowsView,
    flow: &Flow,
    state: &FlowState,
    window: &mut Window,
    cx: &mut Context<NetworkFlowsView>,
) -> AnyElement {
    let _ = window;
    let colors = cx.colors().clone();
    let kind = match state {
        FlowState::Ready { kind, .. } => Some(*kind),
        _ => None,
    };
    let weak = cx.entity().downgrade();
    let close = {
        let weak = weak.clone();
        kubyl_ui::IconButton::new("flow-details-close", IconName::X)
            .icon_size(13.0)
            .on_click(move |_, _, cx| {
                weak.update(cx, |this, cx| this.set_details_open(false, cx))
                    .ok();
            })
    };
    let head = h_flex()
        .flex_none()
        .h(u(34.0))
        .px(u(12.0))
        .border_b_1()
        .border_color(colors.border_variant)
        .child(div().flex_1().font_weight(FontWeight::MEDIUM).child("Flow"))
        .child(close);

    let mut chips = vec![widgets::verdict_pill(flow.verdict, &colors)];
    chips.push(Chip::new(flow.protocol_label_short()).into_any_element());
    if flow.direction != crate::model::Direction::Unknown {
        chips.push(
            Chip::new(match flow.direction {
                crate::model::Direction::Ingress => "ingress",
                _ => "egress",
            })
            .into_any_element(),
        );
    }
    let mut when = widgets::local_and_utc(flow.time);
    if let Some(event) = &flow.event {
        when.push_str(&format!(" · {event}"));
    }
    let connection = {
        let weak = weak.clone();
        widgets::button(
            "flow-filter-connection",
            Some(IconName::Funnel),
            "This connection",
            false,
            &colors,
            move |_, _, cx| {
                weak.update(cx, |this, cx| {
                    this.filter_to_selected(SelectedPart::Connection, cx)
                })
                .ok();
            },
        )
    };
    let copy = (kind == Some(BackendKind::Hubble)).then(|| {
        let command = hubble_command(flow);
        widgets::button(
            "flow-copy-hubble",
            Some(IconName::Copy),
            "Copy as hubble observe",
            false,
            &colors,
            move |_, _, cx| {
                cx.write_to_clipboard(ClipboardItem::new_string(command.clone()));
                NotificationCenter::push(
                    cx,
                    Notification::success("Copied the hubble observe command."),
                );
            },
        )
    });
    let top = v_flex()
        .px(u(14.0))
        .py(u(12.0))
        .gap(u(6.0))
        .border_b_1()
        .border_color(colors.border_variant)
        .child(h_flex().gap(u(8.0)).flex_wrap().children(chips))
        .child(
            div()
                .text_size(u(12.0))
                .text_color(colors.text_muted)
                .child(when),
        )
        .child(
            h_flex()
                .gap(u(6.0))
                .mt(u(4.0))
                .child(connection)
                .children(copy),
        );

    let mut sections: Vec<AnyElement> = vec![top.into_any_element()];
    if let Some(policy) = policy_section(view, flow, &colors, cx) {
        sections.push(policy);
    }
    sections.push(endpoint_section(
        "Source",
        "src",
        &flow.source,
        view,
        &colors,
        cx,
    ));
    sections.push(endpoint_section(
        "Destination",
        "dst",
        &flow.destination,
        view,
        &colors,
        cx,
    ));
    if let Some(l7) = &flow.l7 {
        sections.push(l7_section(l7, &colors));
    }
    let mut counters: Vec<(SharedString, AnyElement)> = Vec::new();
    if let Some(bytes) = flow.bytes {
        counters.push((
            "Bytes".into(),
            widgets::mono(widgets::bytes(bytes), &colors).into_any_element(),
        ));
    }
    if let Some(packets) = flow.packets {
        counters.push((
            "Packets".into(),
            widgets::mono(widgets::count(packets as usize), &colors).into_any_element(),
        ));
    }
    if let Some(flags) = &flow.tcp_flags {
        counters.push((
            "TCP flags".into(),
            widgets::mono(flags.to_string(), &colors).into_any_element(),
        ));
    }
    if let Some(start) = flow.start {
        counters.push((
            "Interval".into(),
            div()
                .text_color(colors.text_muted)
                .child(format!(
                    "{}s from {}",
                    flow.time.duration_since(start).as_secs().max(0),
                    widgets::local_and_utc(start)
                ))
                .into_any_element(),
        ));
    }
    if !counters.is_empty() {
        sections.push(
            widgets::section("Counters", &colors)
                .child(widgets::kv(counters, 84.0, &colors))
                .into_any_element(),
        );
    }
    if !flow.raw.is_empty() {
        let title = match kind {
            Some(BackendKind::Hubble) => "Hubble fields",
            Some(BackendKind::Whisker) => "Whisker fields",
            Some(BackendKind::NetObserv) => "NetObserv fields",
            None => "Backend fields",
        };
        sections.push(
            widgets::section(title, &colors)
                .border_b_0()
                .child(
                    v_flex()
                        .gap(u(4.0))
                        .font_family(fonts::MONO)
                        .text_size(u(11.0))
                        .children(flow.raw.iter().map(|(key, value)| {
                            h_flex()
                                .items_start()
                                .gap(u(8.0))
                                .child(
                                    div()
                                        .flex_none()
                                        .w(u(136.0))
                                        .truncate()
                                        .text_color(colors.text_dim)
                                        .child(key.to_string()),
                                )
                                .child(div().flex_1().min_w_0().child(value.clone()))
                        })),
                )
                .into_any_element(),
        );
    }
    v_flex()
        .flex_none()
        .w(u(352.0))
        .h_full()
        .bg(colors.panel)
        .border_l_1()
        .border_color(colors.border)
        .child(head)
        .child(
            v_flex()
                .id("flow-details-body")
                .flex_1()
                .min_h_0()
                .overflow_y_scroll()
                .children(sections),
        )
        .into_any_element()
}

fn policy_section(
    view: &NetworkFlowsView,
    flow: &Flow,
    colors: &Colors,
    cx: &App,
) -> Option<AnyElement> {
    let summary = flow.policies.summary(flow.verdict);
    // What the title names, as a link when the cluster serves the policy's kind.
    let (icon_color, verb, named, detail): (gpui::Hsla, &str, Option<&PolicyRef>, Option<String>) = match &summary {
        PolicySummary::None => return None,
        PolicySummary::DeniedBy(_) => {
            let policy = flow.policies.denied_by.first()?;
            let mut detail = policy.kind.to_string();
            match flow.direction {
                Direction::Ingress => detail.push_str(" · ingress deny rule"),
                Direction::Egress => detail.push_str(" · egress deny rule"),
                Direction::Unknown => {}
            }
            if let Some(tier) = &policy.tier {
                detail.push_str(&format!(" · tier {tier}"));
            }
            (colors.red, "Denied by", Some(policy), Some(detail))
        }
        PolicySummary::Isolated(named) => (
            colors.red,
            if named.is_some() { "Isolated by" } else { "Denied: isolated, no policy allows it" },
            flow.policies.isolated_by.first(),
            Some(match named {
                Some(_) => "The policy selects the destination and nothing allows this flow: a NetworkPolicy isolates, it has no deny rules.".into(),
                None => "A NetworkPolicy selects the destination and none allows this flow. Plain NetworkPolicies have no deny rules, so the backend can't name one.".into(),
            }),
        ),
        PolicySummary::AllowedBy(_) => {
            let policy = flow.policies.allowed_by.first()?;
            (colors.green, "Allowed by", Some(policy), Some(policy.kind.to_string()))
        }
        PolicySummary::Reason(reason) => (colors.red, reason.as_str(), None, None),
    };
    let mut title = h_flex()
        .flex_wrap()
        .gap(u(4.0))
        .child(div().child(capitalize(verb)));
    if let Some(policy) = named {
        let label = policy.label();
        // More than one policy: the rest follow as plain text.
        let more = match &summary {
            PolicySummary::DeniedBy(names)
            | PolicySummary::AllowedBy(names)
            | PolicySummary::Isolated(Some(names)) => names
                .strip_prefix(label.as_str())
                .filter(|rest| !rest.is_empty())
                .map(str::to_string),
            _ => None,
        };
        title = title.child(match widgets::policy_target(&view.cluster, policy, cx) {
            Some(target) => widgets::link("flow-policy", label, colors, move |_, window, cx| {
                window.dispatch_action(
                    Box::new(OpenView(ViewRequest::for_resource(
                        ViewKind::Details,
                        target.clone(),
                    ))),
                    cx,
                );
            })
            .into_any_element(),
            None => div().child(label).into_any_element(),
        });
        if let Some(more) = more {
            title = title.child(div().child(more));
        }
    }
    Some(
        widgets::section("Policy", colors)
            .child(
                h_flex()
                    .items_start()
                    .gap(u(8.0))
                    .text_size(u(12.5))
                    .child(Icon::new(IconName::Shield).size(14.0).color(icon_color))
                    .child(
                        v_flex()
                            .flex_1()
                            .min_w_0()
                            .gap(u(2.0))
                            .child(title)
                            .when_some(detail, |this, detail| {
                                this.child(
                                    div()
                                        .text_size(u(12.0))
                                        .text_color(colors.text_dim)
                                        .child(detail),
                                )
                            }),
                    ),
            )
            .into_any_element(),
    )
}

fn capitalize(text: &str) -> String {
    let mut chars = text.chars();
    match chars.next() {
        Some(first) => first.to_uppercase().chain(chars).collect(),
        None => String::new(),
    }
}

fn endpoint_section(
    title: &'static str,
    id: &'static str,
    endpoint: &Endpoint,
    view: &NetworkFlowsView,
    colors: &Colors,
    cx: &mut Context<NetworkFlowsView>,
) -> AnyElement {
    let mut rows: Vec<(SharedString, AnyElement)> = Vec::new();
    let openable = endpoint.kind == EndpointKind::Pod
        && endpoint.pod.as_deref().is_some_and(|p| !p.ends_with('*'));
    let head: AnyElement = match (&endpoint.namespace, &endpoint.pod) {
        (Some(ns), Some(pod)) if openable => {
            let target = ResourceRef::object(
                view.cluster.clone(),
                Gvr::new("", "v1", "pods"),
                Some(ns.to_string()),
                pod.to_string(),
            );
            h_flex()
                .gap(u(7.0))
                .child(Icon::new(IconName::Box).size(13.0).color(colors.accent))
                .child(
                    widgets::link(
                        SharedString::from(format!("flow-{id}-pod")),
                        format!("{ns}/{pod}"),
                        colors,
                        move |_, window, cx| {
                            window.dispatch_action(
                                Box::new(OpenView(ViewRequest::for_resource(
                                    ViewKind::Details,
                                    target.clone(),
                                ))),
                                cx,
                            );
                        },
                    )
                    .font_family(fonts::MONO)
                    .text_size(u(11.5)),
                )
                .into_any_element()
        }
        _ => super::table::endpoint_cell(endpoint, colors),
    };
    if let Some(workload) = &endpoint.workload {
        rows.push((
            "Workload".into(),
            div()
                .child(format!(
                    "{} {}",
                    workload.kind,
                    workload.name.trim_end_matches("-*")
                ))
                .into_any_element(),
        ));
    }
    if endpoint.namespace.is_none() && endpoint.kind != EndpointKind::Pod {
        rows.push((
            "Kind".into(),
            div().child(endpoint.kind.label()).into_any_element(),
        ));
    }
    if let Some(ip) = endpoint.ip {
        let address = match endpoint.port {
            Some(port) if ip.is_ipv6() => format!("[{ip}]:{port}"),
            Some(port) => format!("{ip}:{port}"),
            None => ip.to_string(),
        };
        rows.push((
            "Address".into(),
            widgets::mono(address, colors).into_any_element(),
        ));
    } else if let Some(port) = endpoint.port {
        rows.push((
            "Port".into(),
            widgets::mono(port.to_string(), colors).into_any_element(),
        ));
    }
    if let Some(service) = &endpoint.service {
        rows.push((
            "Service".into(),
            widgets::mono(service.to_string(), colors).into_any_element(),
        ));
    }
    if let Some(node) = &endpoint.node {
        rows.push((
            "Node".into(),
            widgets::mono(node.to_string(), colors).into_any_element(),
        ));
    }
    if !endpoint.names.is_empty() {
        rows.push((
            "Names".into(),
            widgets::mono(
                endpoint
                    .names
                    .iter()
                    .map(|n| n.as_ref())
                    .collect::<Vec<_>>()
                    .join(", "),
                colors,
            )
            .into_any_element(),
        ));
    }
    let filter = {
        let weak = cx.entity().downgrade();
        let part = if id == "src" {
            SelectedPart::Source
        } else {
            SelectedPart::Destination
        };
        kubyl_ui::IconButton::new(
            SharedString::from(format!("flow-filter-{id}")),
            IconName::Funnel,
        )
        .icon_size(12.0)
        .on_click(move |_, _, cx| {
            weak.update(cx, |this, cx| this.filter_to_selected(part, cx))
                .ok();
        })
    };
    widgets::section(title, colors)
        .child(
            h_flex()
                .gap(u(6.0))
                .child(div().flex_1().min_w_0().child(head))
                .child(filter),
        )
        .when(!rows.is_empty(), |this| {
            this.child(widgets::kv(rows, 70.0, colors))
        })
        .when(!endpoint.labels.is_empty(), |this| {
            this.child(
                h_flex().flex_wrap().gap(u(4.0)).children(
                    endpoint
                        .labels
                        .iter()
                        .take(8)
                        .map(|l| Chip::new(l.to_string()).mono()),
                ),
            )
        })
        .into_any_element()
}

fn l7_section(l7: &L7, colors: &Colors) -> AnyElement {
    let mut rows: Vec<(SharedString, AnyElement)> = Vec::new();
    let title = match l7 {
        L7::Http {
            method,
            url,
            code,
            protocol,
            latency_ms,
            response,
            headers,
        } => {
            rows.push((
                "Request".into(),
                widgets::mono(format!("{method} {url}"), colors).into_any_element(),
            ));
            if let Some(code) = code {
                rows.push((
                    "Status".into(),
                    widgets::mono(code.to_string(), colors).into_any_element(),
                ));
            }
            if let Some(protocol) = protocol {
                rows.push((
                    "Protocol".into(),
                    div().child(protocol.to_string()).into_any_element(),
                ));
            }
            if let Some(latency) = latency_ms {
                rows.push((
                    "Latency".into(),
                    div().child(format!("{latency:.2} ms")).into_any_element(),
                ));
            }
            for (name, value) in headers.iter().take(12) {
                rows.push((
                    name.clone().into(),
                    widgets::mono(value.clone(), colors).into_any_element(),
                ));
            }
            if *response {
                "HTTP response"
            } else {
                "HTTP request"
            }
        }
        L7::Dns {
            query,
            rcode,
            answers,
            response,
        } => {
            rows.push((
                "Query".into(),
                widgets::mono(query.clone(), colors).into_any_element(),
            ));
            if let Some(rcode) = rcode {
                rows.push((
                    "Answer".into(),
                    div().child(rcode.to_string()).into_any_element(),
                ));
            }
            if !answers.is_empty() {
                rows.push((
                    "Addresses".into(),
                    widgets::mono(answers.join(", "), colors).into_any_element(),
                ));
            }
            if *response {
                "DNS response"
            } else {
                "DNS query"
            }
        }
        L7::Other { protocol, summary } => {
            rows.push((
                protocol.to_string().into(),
                widgets::mono(summary.clone(), colors).into_any_element(),
            ));
            "Application layer"
        }
    };
    widgets::section(title, colors)
        .child(widgets::kv(rows, 84.0, colors))
        .child(
            div()
                .text_size(u(11.0))
                .text_color(colors.text_faint)
                .child("Query values and credentials are hidden (netflow.keep_query_values)."),
        )
        .into_any_element()
}

/// `hubble observe --from-pod payments/a --to-pod payments/b --port 80 --verdict DROPPED`.
fn hubble_command(flow: &Flow) -> String {
    let mut command = String::from("hubble observe");
    let side = |endpoint: &Endpoint, pod_flag: &str, ip_flag: &str| match (
        &endpoint.namespace,
        &endpoint.pod,
        endpoint.ip,
    ) {
        (Some(ns), Some(pod), _) => format!(" {pod_flag} {ns}/{pod}"),
        (_, _, Some(ip)) => format!(" {ip_flag} {ip}"),
        _ => String::new(),
    };
    command.push_str(&side(&flow.source, "--from-pod", "--from-ip"));
    command.push_str(&side(&flow.destination, "--to-pod", "--to-ip"));
    if let Some(port) = flow.destination.port {
        command.push_str(&format!(" --to-port {port}"));
    }
    command.push_str(&format!(
        " --verdict {}",
        flow.verdict.label().to_uppercase().replace(' ', "_")
    ));
    command
}

impl Flow {
    /// `TCP :80` (without the L7 summary).
    fn protocol_label_short(&self) -> String {
        match self.destination.port {
            Some(port) => format!("{} :{port}", self.protocol.label()),
            None => self.protocol.label(),
        }
    }
}
