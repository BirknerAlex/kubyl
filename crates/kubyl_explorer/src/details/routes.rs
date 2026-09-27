//! The Summary of an OpenShift Route: what each router made of it (header pills and the Routers
//! section), host and URL, TLS with the inline key masked until revealed (like Secret data),
//! the backend Services with their weights, resolved ports and one-click forwards, and their
//! endpoints. The pods behind the backends show in the shared Pods section.

use std::sync::Arc;

use gpui::{AnyElement, ClipboardItem, Context, IntoElement, SharedString, div, prelude::*};
use kubyl_core::{Gvr, Notification, NotificationCenter, ResourceRef, Tone};
use kubyl_resources::format::{human_duration, seconds_since, str_at, timestamp};
use kubyl_resources::object_key;
use kubyl_resources::route::{self, Route, ServicePortMatch};
use kubyl_ui::{Chip, Colors, Icon, IconButton, IconName, StatusPill, fonts, h_flex, u, v_flex};
use serde_json::Value;

use super::{DetailsContent, PortRow, SECRET_MASK, Target, chips, link, row_button, section};

/// The `revealed` entry of the inline key.
pub(super) const KEY_REVEAL: &str = "route:spec.tls.key";

/// Admitted / not admitted per router, then the TLS chip.
pub(super) fn route_pills(route: &Route, mut pills: gpui::Div) -> gpui::Div {
    if route.routers.is_empty() {
        pills = pills.child(StatusPill::new("Not admitted yet", Tone::Warning));
    }
    for router in &route.routers {
        let (label, tone) = if router.is_admitted() {
            (format!("Admitted · {}", router.router), Tone::Good)
        } else if router.is_rejected() {
            (format!("Not admitted · {}", router.router), Tone::Bad)
        } else {
            (format!("Pending · {}", router.router), Tone::Warning)
        };
        pills = pills.child(StatusPill::new(label, tone));
    }
    pills.child(Chip::new(match &route.tls {
        Some(tls) => tls.label(),
        None => "no TLS".to_string(),
    }))
}

/// A label and a value element, aligned like the details' key/value rows.
fn kv_row(label: &'static str, value: impl IntoElement, colors: &Colors) -> impl IntoElement {
    h_flex()
        .gap(u(8.0))
        .min_h(u(20.0))
        .text_size(u(12.0))
        .child(
            div()
                .flex_none()
                .w(u(104.0))
                .text_color(colors.text_dim)
                .child(label),
        )
        .child(div().flex_1().min_w_0().child(value))
}

fn text(value: impl Into<SharedString>, color: gpui::Hsla) -> gpui::Div {
    div().truncate().text_color(color).child(value.into())
}

/// `10.244.0.12:8080` for each ready endpoint of `service` in its EndpointSlices. An endpoint
/// without a ready condition counts as ready (the API's advice for an unknown state).
pub(super) fn ready_endpoints<'a>(
    slices: impl IntoIterator<Item = &'a Arc<Value>>,
    service: &str,
) -> Vec<String> {
    let mut out = Vec::new();
    for slice in slices {
        if str_at(slice, "/metadata/labels/kubernetes.io~1service-name") != service {
            continue;
        }
        let ports: Vec<i64> = kubyl_resources::format::array_at(slice, "/ports")
            .iter()
            .filter_map(|p| p["port"].as_i64())
            .collect();
        for endpoint in kubyl_resources::format::array_at(slice, "/endpoints") {
            if endpoint
                .pointer("/conditions/ready")
                .and_then(Value::as_bool)
                == Some(false)
            {
                continue;
            }
            for address in kubyl_resources::format::array_at(endpoint, "/addresses") {
                let Some(address) = address.as_str() else {
                    continue;
                };
                if ports.is_empty() {
                    out.push(address.to_string());
                }
                out.extend(ports.iter().map(|p| format!("{address}:{p}")));
            }
        }
    }
    out.sort();
    out.dedup();
    out
}

/// `1 endpoint ready`, `3 endpoints ready`, `no ready endpoints`.
fn endpoints_label(count: usize) -> String {
    match count {
        0 => "no ready endpoints".into(),
        1 => "1 endpoint ready".into(),
        n => format!("{n} endpoints ready"),
    }
}

impl DetailsContent {
    /// A backend Service of the shown Route, once the namespace's Services loaded.
    fn route_service(&self, name: &str, cx: &gpui::App) -> Option<Arc<Value>> {
        let namespace = self.target.as_ref()?.namespace.as_deref();
        self.related
            .services
            .as_ref()?
            .read(cx)
            .get(&object_key(namespace, name))
            .cloned()
    }

    /// The Service port the Route's target port selects on its backend `service`, like the
    /// router (with the backend's EndpointSlices once they loaded).
    fn route_port(
        &self,
        route: &Route,
        service: &Value,
        cx: &gpui::App,
    ) -> Result<ServicePortMatch, String> {
        let name = str_at(service, "/metadata/name");
        let endpoints = self
            .related
            .endpoint_slices
            .as_ref()
            .map(|s| route::endpoint_ports(s.read(cx).objects().values().map(|o| &**o), name))
            .unwrap_or_default();
        route::resolve_target_port_with(route.target_port.as_ref(), service, &endpoints)
    }

    /// The label selectors of a Route's backend Services (for the Pods section).
    pub(super) fn route_selectors(
        &self,
        object: &Value,
        cx: &gpui::App,
    ) -> Vec<Vec<(String, String)>> {
        Route::parse(object)
            .services()
            .filter_map(|backend| {
                let service = self.route_service(&backend.name, cx)?;
                super::selector_of("Service", &service)
            })
            .collect()
    }

    pub(super) fn render_route(
        &self,
        object: &Value,
        target: &Target,
        colors: &Colors,
        cx: &mut Context<Self>,
    ) -> Vec<AnyElement> {
        let route = Route::parse(object);
        let now = jiff::Timestamp::now();
        let mut out = vec![self.route_section(&route, colors, cx).into_any_element()];

        // Routers.
        let mut routers = v_flex().gap(u(8.0)).text_size(u(12.0));
        if route.routers.is_empty() {
            routers = routers.child(text(
                "No router has reported on this Route yet.",
                colors.text_dim,
            ));
        }
        for router in &route.routers {
            let admission = router.admitted.as_ref();
            let (icon, color) = if router.is_admitted() {
                (IconName::CircleCheck, colors.green)
            } else if router.is_rejected() {
                (IconName::CircleX, colors.red)
            } else {
                (IconName::Clock, colors.yellow)
            };
            let age = admission
                .and_then(|a| a.last_transition.as_deref())
                .and_then(timestamp)
                .map(|t| human_duration(seconds_since(t, now)))
                .unwrap_or_default();
            let verdict = match admission {
                Some(a) if a.status == "True" => "admitted".to_string(),
                Some(a) => match &a.reason {
                    Some(reason) => format!("not admitted: {reason}"),
                    None => "not admitted".into(),
                },
                None => "no Admitted condition yet".into(),
            };
            let mut details = vec![format!(
                "host {}",
                router.host.as_deref().unwrap_or("<none>")
            )];
            if let Some(canonical) = &router.canonical_hostname {
                details.push(format!("router {canonical}"));
            }
            if let Some(policy) = &router.wildcard_policy {
                details.push(format!("wildcard {policy}"));
            }
            routers = routers.child(
                h_flex()
                    .items_start()
                    .gap(u(8.0))
                    .child(
                        div()
                            .pt(u(2.0))
                            .child(Icon::new(icon).size(12.0).color(color)),
                    )
                    .child(
                        v_flex()
                            .flex_1()
                            .min_w_0()
                            .child(
                                h_flex()
                                    .gap(u(8.0))
                                    .child(
                                        div()
                                            .font_family(fonts::MONO)
                                            .text_size(u(11.5))
                                            .child(router.router.clone()),
                                    )
                                    .child(text(verdict, color))
                                    .child(div().flex_1())
                                    .child(
                                        div()
                                            .font_family(fonts::MONO)
                                            .text_size(u(11.0))
                                            .text_color(colors.text_dim)
                                            .child(age),
                                    ),
                            )
                            .child(text(details.join(" · "), colors.text_dim))
                            .when_some(
                                admission
                                    .and_then(|a| a.message.clone())
                                    .filter(|_| !router.is_admitted()),
                                |this, message| {
                                    this.child(div().text_color(colors.text_muted).child(message))
                                },
                            ),
                    ),
            );
        }
        out.push(section("Routers", colors).child(routers).into_any_element());

        if let Some(tls) = &route.tls {
            out.push(self.tls_section(object, tls, target, colors, cx));
        }
        out.push(self.backends_section(&route, target, colors, cx));
        if let Some(endpoints) = self.endpoints_section(&route, colors, cx) {
            out.push(endpoints);
        }
        out
    }

    /// Host(s), path, URL with "Open in browser", TLS, wildcard policy, target port.
    fn route_section(&self, route: &Route, colors: &Colors, cx: &gpui::App) -> impl IntoElement {
        let mut hosts: Vec<String> = route.display_host().into_iter().collect();
        for router in route.routers.iter().filter(|r| r.is_admitted()) {
            if let Some(host) = &router.host
                && !hosts.contains(host)
                && route.host.as_deref() != Some(host.as_str())
            {
                hosts.push(format!("{host} (admitted by {})", router.router));
            }
        }
        let mono = |value: String| {
            div()
                .truncate()
                .font_family(fonts::MONO)
                .text_size(u(11.5))
                .text_color(colors.text)
                .child(value)
        };
        let mut body = v_flex().gap(u(5.0)).child(kv_row(
            if hosts.len() > 1 { "Hosts" } else { "Host" },
            if hosts.is_empty() {
                text("none yet", colors.text_dim).into_any_element()
            } else {
                mono(hosts.join(", ")).into_any_element()
            },
            colors,
        ));
        if let Some(path) = &route.path {
            body = body.child(kv_row("Path", mono(path.clone()), colors));
        }
        let url = route::url(route);
        body = body.child(kv_row(
            "URL",
            match (&url, route.is_wildcard()) {
                (Some(url), _) => {
                    let open = url.clone();
                    h_flex()
                        .gap(u(8.0))
                        .child(
                            div()
                                .id("route-url")
                                .min_w_0()
                                .truncate()
                                .font_family(fonts::MONO)
                                .text_size(u(11.5))
                                .text_color(colors.accent)
                                .cursor_pointer()
                                .hover(|s| s.underline())
                                .child(url.clone())
                                .on_click(move |_, _, cx| cx.open_url(&open)),
                        )
                        .child({
                            let open = url.clone();
                            row_button(
                                "route-open-browser",
                                IconName::ExternalLink,
                                "Open in browser",
                                colors,
                            )
                            .on_click(move |_, _, cx| cx.open_url(&open))
                        })
                        .into_any_element()
                }
                (None, true) => {
                    text("No browser link for wildcard hosts", colors.text_dim).into_any_element()
                }
                (None, false) => text("No host yet", colors.text_dim).into_any_element(),
            },
            colors,
        ));
        body = body.child(kv_row(
            "TLS",
            text(
                route
                    .tls
                    .as_ref()
                    .map_or_else(|| "none".to_string(), |t| t.describe()),
                colors.text,
            ),
            colors,
        ));
        body = body.child(kv_row(
            "Wildcard policy",
            text(route.wildcard_policy.clone(), colors.text),
            colors,
        ));
        // `http → Service port 80 → 8080`, against the primary backend.
        let primary = route
            .services()
            .next()
            .and_then(|b| self.route_service(&b.name, cx));
        let target = route.target_port_label();
        let resolved = primary
            .as_ref()
            .map(|service| self.route_port(route, service, cx));
        let port = match (&route.target_port, resolved) {
            (_, Some(Ok(m))) => {
                let mut label = match &route.target_port {
                    Some(port) => format!("{port} → Service port {}", m.port),
                    None => format!("all ports · first Service port {}", m.port),
                };
                if m.target != m.port.to_string() && m.target != target {
                    label.push_str(&format!(" → {}", m.target));
                }
                text(label, colors.text)
            }
            (_, Some(Err(err))) => text(err, colors.yellow),
            (None, None) => text("all ports", colors.text),
            (Some(_), None) => text(target, colors.text),
        };
        body = body.child(kv_row("Target port", port, colors));
        section("Route", colors).child(body)
    }

    /// Termination, insecure policy and which certificates are set; the key masked until
    /// revealed, and copied only by its button.
    fn tls_section(
        &self,
        object: &Value,
        tls: &route::Tls,
        target: &Target,
        colors: &Colors,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let present = |set: bool, unset: &'static str| {
            if set {
                text("present", colors.text)
            } else {
                text(unset, colors.text_dim)
            }
        };
        let mut body = v_flex()
            .gap(u(5.0))
            .child(kv_row(
                "Termination",
                text(tls.termination.clone(), colors.text),
                colors,
            ))
            .child(kv_row(
                "Insecure traffic",
                text(
                    tls.insecure_policy
                        .clone()
                        .unwrap_or_else(|| "not set".into()),
                    colors.text,
                ),
                colors,
            ));
        if tls.termination != "passthrough" {
            body = body.child(kv_row(
                "Certificate",
                present(tls.certificate, "the router's default"),
                colors,
            ));
        }
        let key = route::inline_key(object).map(String::from);
        body = body.child(kv_row(
            "Key",
            match key {
                Some(key) => self.key_row(key, target, colors, cx).into_any_element(),
                None => text("not set", colors.text_dim).into_any_element(),
            },
            colors,
        ));
        body = body.child(kv_row(
            "CA certificate",
            present(tls.ca_certificate, "not set"),
            colors,
        ));
        if tls.termination == "reencrypt" {
            body = body.child(kv_row(
                "Destination CA",
                present(tls.destination_ca_certificate, "the service CA"),
                colors,
            ));
        }
        section("TLS", colors).child(body).into_any_element()
    }

    /// The inline key: masked with reveal and copy, or the revealed PEM.
    fn key_row(
        &self,
        key: String,
        target: &Target,
        colors: &Colors,
        cx: &mut Context<Self>,
    ) -> impl IntoElement {
        let revealed = self.revealed.contains(KEY_REVEAL);
        let reveal = IconButton::new(
            "route-key-reveal",
            if revealed {
                IconName::EyeOff
            } else {
                IconName::Eye
            },
        )
        .icon_size(12.0)
        .toggled(revealed)
        .on_click(cx.listener(|this, _, _, cx| {
            if !this.revealed.remove(KEY_REVEAL) {
                this.revealed.insert(KEY_REVEAL.to_string());
            }
            cx.notify();
        }));
        let name = target.name.clone();
        let copy = IconButton::new("route-key-copy", IconName::Copy)
            .icon_size(12.0)
            .on_click({
                let key = key.clone();
                move |_, _, cx| {
                    cx.stop_propagation();
                    cx.write_to_clipboard(ClipboardItem::new_string(key.clone()));
                    NotificationCenter::push(
                        cx,
                        Notification::info(format!("Copied the TLS key of {name}")),
                    );
                }
            });
        let buttons = h_flex()
            .gap(u(2.0))
            .child(
                div()
                    .debug_selector(|| "route-key-reveal".into())
                    .child(reveal),
            )
            .child(copy);
        if revealed {
            v_flex()
                .debug_selector(|| "route-key-revealed".into())
                .gap(u(4.0))
                .child(buttons)
                .child(
                    div()
                        .p(u(6.0))
                        .rounded(u(4.0))
                        .border_1()
                        .border_color(colors.border_variant)
                        .bg(colors.subheader_background)
                        .font_family(fonts::MONO)
                        .text_size(u(11.0))
                        .children(key.lines().map(|line| div().child(line.to_string()))),
                )
                .into_any_element()
        } else {
            h_flex()
                .debug_selector(|| "route-key-masked".into())
                .gap(u(8.0))
                .child(
                    div()
                        .font_family(fonts::MONO)
                        .text_color(colors.text_dim)
                        .child(SECRET_MASK),
                )
                .child(buttons)
                .into_any_element()
        }
    }

    /// Each backend Service: a link, its share, the port the target port resolves to with a
    /// forward (running forwards show there), and how many endpoints are ready.
    fn backends_section(
        &self,
        route: &Route,
        target: &Target,
        colors: &Colors,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let loaded = self
            .related
            .services
            .as_ref()
            .map(|s| s.read(cx).status().is_settled());
        let slices: Vec<Arc<Value>> = self
            .related
            .endpoint_slices
            .as_ref()
            .map(|s| s.read(cx).objects().values().cloned().collect())
            .unwrap_or_default();
        let weights = route::weights(route);
        let mut list = v_flex().gap(u(10.0)).text_size(u(12.0));
        if route.backends.is_empty() {
            list = list.child(text("No backends.", colors.yellow));
        }
        for (ix, (backend, (_, percent))) in route.backends.iter().zip(weights).enumerate() {
            let share = percent.map(|p| format!("{p}%")).unwrap_or_default();
            if !backend.is_service() {
                list = list.child(
                    h_flex()
                        .gap(u(8.0))
                        .child(text(
                            format!("{} {}", backend.kind, backend.name),
                            colors.text,
                        ))
                        .child(text(share, colors.text_dim)),
                );
                continue;
            }
            let reference = ResourceRef::object(
                target.cluster.clone(),
                Gvr::new("", "v1", "services"),
                target.namespace.clone(),
                backend.name.clone(),
            );
            let head = h_flex()
                .gap(u(8.0))
                .child(
                    div()
                        .flex_none()
                        .w(u(60.0))
                        .text_color(colors.text_dim)
                        .child("Service"),
                )
                .child(
                    div()
                        .flex_1()
                        .min_w_0()
                        .font_family(fonts::MONO)
                        .text_size(u(11.5))
                        .child(link(
                            SharedString::from(format!("route-svc-{ix}")),
                            backend.name.clone(),
                            reference.clone(),
                            colors,
                        )),
                )
                .when(!share.is_empty(), |this| {
                    this.child(
                        div()
                            .font_family(fonts::MONO)
                            .text_size(u(11.5))
                            .text_color(colors.text_muted)
                            .child(share.clone()),
                    )
                });
            let service = self.route_service(&backend.name, cx);
            let body = match (&service, loaded) {
                (Some(service), _) => match self.route_port(route, service, cx) {
                    Ok(matched) => self.forward_rows(
                        reference,
                        vec![port_row(&matched)],
                        None,
                        &format!("route-{ix}-"),
                        colors,
                        cx,
                    ),
                    Err(err) => text(err, colors.yellow).into_any_element(),
                },
                (None, Some(true)) => text(
                    format!(
                        "Service {} not found in {}",
                        backend.name,
                        target.namespace.as_deref().unwrap_or_default()
                    ),
                    colors.yellow,
                )
                .into_any_element(),
                (None, _) => text("Loading…", colors.text_dim).into_any_element(),
            };
            let ready = ready_endpoints(&slices, &backend.name).len();
            list = list.child(
                v_flex()
                    .gap(u(4.0))
                    .child(head)
                    .child(div().pl(u(68.0)).child(body))
                    .when(self.related.endpoint_slices.is_some(), |this| {
                        this.child(div().pl(u(68.0)).child(text(
                            endpoints_label(ready),
                            if ready == 0 {
                                colors.yellow
                            } else {
                                colors.text_dim
                            },
                        )))
                    }),
            );
        }
        section("Backends", colors).child(list).into_any_element()
    }

    /// The ready endpoints of the backend Services.
    fn endpoints_section(
        &self,
        route: &Route,
        colors: &Colors,
        cx: &gpui::App,
    ) -> Option<AnyElement> {
        let store = self.related.endpoint_slices.as_ref()?.read(cx);
        let slices: Vec<&Arc<Value>> = store.objects().values().collect();
        let mut total = 0;
        let mut list = v_flex().gap(u(6.0)).text_size(u(12.0));
        for backend in route.services() {
            let addresses = ready_endpoints(slices.iter().copied(), &backend.name);
            total += addresses.len();
            list = list.child(
                v_flex()
                    .gap(u(3.0))
                    .child(text(backend.name.clone(), colors.text_dim))
                    .child(if addresses.is_empty() {
                        text("No ready endpoints.", colors.yellow).into_any_element()
                    } else {
                        chips(addresses, true).into_any_element()
                    }),
            );
        }
        Some(
            section(format!("Endpoints · {total} ready"), colors)
                .child(list)
                .into_any_element(),
        )
    }
}

/// A resolved Service port as a forwardable row.
fn port_row(matched: &ServicePortMatch) -> PortRow {
    PortRow {
        port: matched.port,
        label: matched.label(),
        detail: matched.name.clone().unwrap_or_default(),
        tcp: matched.protocol == "TCP",
    }
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    use super::*;

    #[test]
    fn ready_endpoints_per_service() {
        let slice = |service: &str, endpoints: Value| {
            Arc::new(json!({
                "metadata": {"labels": {"kubernetes.io/service-name": service}},
                "ports": [{"name": "http", "port": 8080}],
                "endpoints": endpoints
            }))
        };
        let slices = [
            slice(
                "shop-web",
                json!([
                    {"addresses": ["10.0.0.2"], "conditions": {"ready": true}},
                    {"addresses": ["10.0.0.1"]},
                    {"addresses": ["10.0.0.3"], "conditions": {"ready": false}}
                ]),
            ),
            slice("shop-canary", json!([])),
        ];
        assert_eq!(
            ready_endpoints(&slices, "shop-web"),
            ["10.0.0.1:8080", "10.0.0.2:8080"]
        );
        assert!(ready_endpoints(&slices, "shop-canary").is_empty());
        assert_eq!(endpoints_label(0), "no ready endpoints");
        assert_eq!(endpoints_label(1), "1 endpoint ready");
        assert_eq!(endpoints_label(2), "2 endpoints ready");
    }
}
