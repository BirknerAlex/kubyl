//! The Gateway API in the details: a Gateway's listeners with their state and attached-route
//! counts, its addresses and the routes attached to it; a GatewayClass's Gateways; a route's
//! parents (accepted or not, per parent) and rules with their matches and backends. A Service
//! gets a "Routes" section (every route sending traffic to it, across namespaces where a
//! ReferenceGrant allows it) and its EndpointSlices; an EndpointSlice lists its endpoints.

use std::sync::Arc;

use gpui::{AnyElement, App, Context, IntoElement, SharedString, div, prelude::*};
use kubyl_core::{Gvr, ResourceRef, Tone};
use kubyl_resources::StoreKey;
use kubyl_resources::columns::{endpoint_counts, endpoint_ports};
use kubyl_resources::format::{array_at, name, namespace, str_at};
use kubyl_resources::gateway::{self, Gateway, Route};
use kubyl_ui::{Chip, Colors, Icon, IconName, StatusDot, h_flex, tone_color, u, v_flex};
use serde_json::Value;

use super::parts::{
    Wrapped, card, empty, kv_row, list, mono_at, mono_link, state, subtitle, text, text_at,
};
use super::{DetailsContent, Target, section};

const LIMIT: usize = 24;

/// The named stores of routes: `routes:<kind>` (the Service's namespace, or what a Gateway's
/// listeners admit) and `routes-from:<kind>:<namespace>` (the namespaces a ReferenceGrant lets
/// send traffic to a Service).
const ROUTES: &str = "routes:";
const ROUTES_FROM: &str = "routes-from:";

fn route_store(kind: &str) -> String {
    format!("{ROUTES}{kind}")
}

/// A route with its parsed form.
type Parsed = (Arc<Value>, Route);

fn sorted(mut routes: Vec<Parsed>) -> Vec<Parsed> {
    routes.sort_by(|(a, ra), (b, rb)| {
        (&ra.kind, namespace(a), name(a)).cmp(&(&rb.kind, namespace(b), name(b)))
    });
    routes
}

fn tone_icon(tone: Tone) -> IconName {
    match tone {
        Tone::Good => IconName::CircleCheck,
        Tone::Bad => IconName::CircleX,
        _ => IconName::Clock,
    }
}

impl DetailsContent {
    pub(super) fn load_gateway(&mut self, target: &Target, object: &Value, cx: &mut Context<Self>) {
        let ns = target.namespace.clone();
        match (target.gvr.group.as_str(), target.kind.as_str()) {
            ("", "Service") => {
                for (kind, resource) in gateway::ROUTE_KINDS {
                    self.watch_named(
                        route_store(kind),
                        target,
                        (gateway::GROUP, resource),
                        ns.clone(),
                        None,
                        cx,
                    );
                }
                if self.watch_named(
                    "grants",
                    target,
                    (gateway::GROUP, "referencegrants"),
                    ns.clone(),
                    None,
                    cx,
                ) && let Some(grants) = self.related.named.get("grants").cloned()
                {
                    let observer = cx.observe(grants.entity(), |this, _, cx| {
                        this.service_grants_changed(cx)
                    });
                    self.related._observers.push(observer);
                    self.service_grants_changed(cx);
                }
                if let Some(ns) = ns {
                    let key = StoreKey::new(
                        target.cluster.clone(),
                        self.served(&target.cluster, "discovery.k8s.io", "endpointslices", cx)
                            .map(|i| i.gvr)
                            .unwrap_or_else(|| {
                                Gvr::new("discovery.k8s.io", "v1", "endpointslices")
                            }),
                        Some(ns),
                    )
                    .labels(format!("kubernetes.io/service-name={}", target.name));
                    let handle = self.acquire(key, cx);
                    self.related.named.insert("endpointslices".into(), handle);
                }
            }
            (gateway::GROUP, "Gateway") => {
                // Only the route kinds the listeners admit, and only the Gateway's namespace
                // unless a listener takes routes from others.
                let (kinds, other_namespaces) = gateway::attachable_routes(object);
                let scope = if other_namespaces { None } else { ns };
                for (kind, resource) in kinds {
                    self.watch_named(
                        route_store(kind),
                        target,
                        (gateway::GROUP, resource),
                        scope.clone(),
                        None,
                        cx,
                    );
                }
            }
            (gateway::GROUP, "GatewayClass") => {
                self.watch_named(
                    "gateways",
                    target,
                    (gateway::GROUP, "gateways"),
                    None,
                    None,
                    cx,
                );
            }
            _ => {}
        }
    }

    /// Routes in other namespaces count for a Service once a ReferenceGrant lets them refer to
    /// it: watch the routes of the kinds and namespaces the grants name.
    fn service_grants_changed(&mut self, cx: &mut Context<Self>) {
        let Some(target) = self.target.clone() else {
            return;
        };
        let own = target.namespace.clone().unwrap_or_default();
        let grants = self.named("grants", cx);
        let sources = gateway::granted_route_sources(grants.iter().map(|g| &**g), &target.name);
        let mut added = false;
        for (kind, resource, namespace) in sources {
            let store = format!("{ROUTES_FROM}{kind}:{namespace}");
            if namespace == own || self.has_named(&store) {
                continue;
            }
            added |= self.watch_named(
                store,
                &target,
                (gateway::GROUP, resource),
                Some(namespace),
                None,
                cx,
            );
        }
        if added {
            cx.notify();
        }
    }

    pub(super) fn render_gateway(
        &mut self,
        object: &Value,
        target: &Target,
        colors: &Colors,
        cx: &mut Context<Self>,
    ) -> Vec<AnyElement> {
        match (target.gvr.group.as_str(), target.kind.as_str()) {
            ("", "Service") => self.render_service_routes(target, colors, cx),
            ("discovery.k8s.io", "EndpointSlice") => render_endpoint_slice(object, target, colors),
            (gateway::GROUP, "Gateway") => self.render_gateway_details(object, target, colors, cx),
            (gateway::GROUP, "GatewayClass") => {
                self.render_gateway_class(object, target, colors, cx)
            }
            (gateway::GROUP, kind) if gateway::is_route(gateway::GROUP, kind) => {
                self.render_route_details(object, target, colors, cx)
            }
            _ => Vec::new(),
        }
    }

    /// Every route in the named stores starting with `prefix` (`routes:<kind>`,
    /// `routes-from:<kind>:<namespace>`), parsed, with its kind filled in from the store name
    /// (watch caches can drop `kind`).
    fn routes_in(&self, prefix: &str, cx: &App) -> Vec<Parsed> {
        let mut out = Vec::new();
        for (store, handle) in &self.related.named {
            let Some(rest) = store.strip_prefix(prefix) else {
                continue;
            };
            let kind = rest.split(':').next().unwrap_or_default();
            for route in handle.read(cx).objects().values() {
                let mut route = route.clone();
                if str_at(&route, "/kind").is_empty() {
                    let mut copy = (*route).clone();
                    copy["kind"] = Value::String(kind.to_string());
                    route = Arc::new(copy);
                }
                let parsed = Route::parse(&route);
                out.push((route, parsed));
            }
        }
        out
    }

    fn route_ref(&self, route: &Value, target: &Target, cx: &App) -> ResourceRef {
        let kind = str_at(route, "/kind");
        let resource = gateway::ROUTE_KINDS
            .iter()
            .find(|(k, _)| *k == kind)
            .map(|(_, r)| *r)
            .unwrap_or("httproutes");
        self.reference(
            target,
            gateway::GROUP,
            resource,
            namespace(route).map(String::from),
            name(route),
            cx,
        )
    }

    fn gateway_ref(&self, ns: &str, gateway_name: &str, target: &Target, cx: &App) -> ResourceRef {
        self.reference(
            target,
            gateway::GROUP,
            "gateways",
            Some(ns.to_string()),
            gateway_name,
            cx,
        )
    }

    fn render_gateway_details(
        &self,
        object: &Value,
        target: &Target,
        colors: &Colors,
        cx: &App,
    ) -> Vec<AnyElement> {
        let gw = Gateway::parse(object);
        let ns = target.namespace.clone().unwrap_or_default();
        let mut facts = v_flex().gap(u(4.0));
        let class = self.reference(
            target,
            gateway::GROUP,
            "gatewayclasses",
            None,
            gw.class.clone(),
            cx,
        );
        facts = facts.child(kv_row(
            "Class",
            80.0,
            mono_link("gateway-class", gw.class.clone(), class, colors),
            colors,
        ));
        facts = facts.child(kv_row(
            "Addresses",
            80.0,
            text(
                if gw.addresses.is_empty() {
                    "none yet".into()
                } else {
                    gw.addresses.join(", ")
                },
                if gw.addresses.is_empty() {
                    colors.text_dim
                } else {
                    colors.text
                },
            ),
            colors,
        ));
        let mut out = vec![section("Gateway", colors).child(facts).into_any_element()];

        let mut listeners = list();
        for (ix, listener) in gw.listeners.iter().enumerate() {
            let (label, tone) = listener.state();
            let mut head = h_flex()
                .gap(u(6.0))
                .child(
                    text_at(ix, listener.name.clone(), colors.text)
                        .font_weight(gpui::FontWeight::MEDIUM),
                )
                .child(
                    Chip::new(format!("{} :{}", listener.protocol, listener.port))
                        .mono()
                        .selectable_as(("listener-port", ix as u64)),
                );
            if !listener.hostname.is_empty() {
                head = head.child(mono_at(ix, listener.hostname.clone(), colors.text_muted));
            }
            head = head.child(div().flex_1()).child(
                h_flex()
                    .gap(u(4.0))
                    .child(
                        Icon::new(tone_icon(tone))
                            .size(12.0)
                            .color(tone_color(tone, colors)),
                    )
                    .child(text_at(ix, label, tone_color(tone, colors)).flex_none()),
            );
            let mut facts = Vec::new();
            match listener.attached_routes {
                Some(1) => facts.push("1 route attached".to_string()),
                Some(n) => facts.push(format!("{n} routes attached")),
                None => {}
            }
            facts.push(format!(
                "routes from {}",
                match listener.allowed_namespaces.as_str() {
                    "Same" => "the same namespace".to_string(),
                    "All" => "all namespaces".to_string(),
                    other => other.to_string(),
                }
            ));
            if let Some(tls) = &listener.tls {
                facts.push(format!("TLS {tls}"));
            }
            let mut card = card(colors).child(head).child(
                text_at(ix, facts.join(" · "), colors.text_dim)
                    .text_size(u(11.5))
                    .wrapped(),
            );
            for (cx_ix, condition) in listener
                .conditions
                .iter()
                .filter(|c| c.is_false() && !c.message.is_empty())
                .enumerate()
            {
                card = card.child(
                    text_at(
                        ix * 100 + cx_ix,
                        format!("{}: {}", condition.kind, condition.message),
                        colors.red,
                    )
                    .text_size(u(11.5))
                    .wrapped(),
                );
            }
            listeners = listeners.child(card);
        }
        if gw.listeners.is_empty() {
            listeners = listeners.child(empty("No listeners.", colors));
        }
        out.push(
            section(format!("Listeners · {}", gw.listeners.len()), colors)
                .child(listeners)
                .into_any_element(),
        );

        if gateway::ROUTE_KINDS
            .iter()
            .any(|(kind, _)| self.has_named(&route_store(kind)))
        {
            let attached = self.derived("gateway-routes", &[ROUTES], cx, |this| {
                sorted(
                    this.routes_in(ROUTES, cx)
                        .into_iter()
                        .filter(|(_, route)| !route.parents_on(&ns, &target.name).is_empty())
                        .collect(),
                )
            });
            let mut body = list().gap(u(4.0));
            if attached.is_empty() {
                body = body.child(empty("No route names this Gateway as a parent.", colors));
            }
            for (ix, (route_object, route)) in attached.iter().take(LIMIT).enumerate() {
                let parents = route.parents_on(&ns, &target.name);
                let tone = parents
                    .iter()
                    .map(|p| {
                        route
                            .status_of(p)
                            .map(|s| s.state().1)
                            .unwrap_or(Tone::Warning)
                    })
                    .min_by_key(|t| match t {
                        Tone::Bad => 0,
                        Tone::Warning => 1,
                        _ => 2,
                    })
                    .unwrap_or(Tone::Warning);
                let sections: Vec<String> = parents
                    .iter()
                    .map(|p| {
                        if p.section.is_empty() {
                            "all listeners".into()
                        } else {
                            p.section.clone()
                        }
                    })
                    .collect();
                let label = if route.namespace == ns {
                    name(route_object).to_string()
                } else {
                    format!("{}/{}", route.namespace, name(route_object))
                };
                body = body.child(
                    h_flex()
                        .gap(u(7.0))
                        .child(
                            Icon::new(tone_icon(tone))
                                .size(12.0)
                                .color(tone_color(tone, colors)),
                        )
                        .child(text_at(ix, route.kind.clone(), colors.text_dim).flex_none())
                        .child(mono_link(
                            format!("attached-{ix}"),
                            label,
                            self.route_ref(route_object, target, cx),
                            colors,
                        ))
                        .child(div().flex_1())
                        .child(
                            text_at(ix, sections.join(", "), colors.text_dim).text_size(u(11.5)),
                        ),
                );
            }
            out.push(
                section(format!("Attached routes · {}", attached.len()), colors)
                    .child(body)
                    .into_any_element(),
            );
        }
        out
    }

    fn render_gateway_class(
        &self,
        object: &Value,
        target: &Target,
        colors: &Colors,
        cx: &App,
    ) -> Vec<AnyElement> {
        let (controller, _) = gateway::gateway_class(object);
        let mut facts = v_flex().gap(u(4.0)).child(kv_row(
            "Controller",
            80.0,
            mono_at(0, controller, colors.text),
            colors,
        ));
        let description = str_at(object, "/spec/description");
        if !description.is_empty() {
            facts = facts.child(kv_row(
                "Description",
                80.0,
                text(description.to_string(), colors.text).wrapped(),
                colors,
            ));
        }
        if let Some(params) = object.pointer("/spec/parametersRef") {
            facts = facts.child(kv_row(
                "Parameters",
                80.0,
                text(
                    format!("{} {}", str_at(params, "/kind"), str_at(params, "/name")),
                    colors.text,
                ),
                colors,
            ));
        }
        let mut out = vec![
            section("Gateway class", colors)
                .child(facts)
                .into_any_element(),
        ];
        if self.has_named("gateways") {
            let gateways = self.derived("class-gateways", &["gateways"], cx, |this| {
                this.named("gateways", cx)
                    .into_iter()
                    .filter(|g| str_at(g, "/spec/gatewayClassName") == target.name)
                    .collect::<Vec<_>>()
            });
            let mut body = list().gap(u(4.0));
            if gateways.is_empty() {
                body = body.child(empty("No Gateway uses this class.", colors));
            }
            for (ix, gw) in gateways.iter().take(LIMIT).enumerate() {
                let (label, tone) = Gateway::parse(gw).state();
                let ns = namespace(gw).unwrap_or_default();
                body = body.child(
                    h_flex()
                        .gap(u(7.0))
                        .child(StatusDot::new(tone_color(tone, colors)))
                        .child(mono_link(
                            format!("class-gateway-{ix}"),
                            format!("{ns}/{}", name(gw)),
                            self.gateway_ref(ns, name(gw), target, cx),
                            colors,
                        ))
                        .child(div().flex_1())
                        .child(text_at(ix, label, tone_color(tone, colors)).flex_none()),
                );
            }
            out.push(
                section(format!("Gateways · {}", gateways.len()), colors)
                    .child(body)
                    .into_any_element(),
            );
        }
        out
    }

    fn render_route_details(
        &self,
        object: &Value,
        target: &Target,
        colors: &Colors,
        cx: &App,
    ) -> Vec<AnyElement> {
        let route = Route::parse(object);
        let mut out = Vec::new();
        if !route.hostnames.is_empty() {
            out.push(
                section("Hostnames", colors)
                    .child(super::chips(route.hostnames.clone(), true))
                    .into_any_element(),
            );
        }

        let mut parents = list();
        if route.parents.is_empty() {
            parents = parents.child(empty(
                "No parents: the route isn't attached to any Gateway.",
                colors,
            ));
        }
        for (ix, parent) in route.parents.iter().enumerate() {
            let status = route.status_of(parent);
            let (label, tone) = status
                .map(|s| s.state())
                .unwrap_or(("Pending".into(), Tone::Warning));
            let parent_label = parent.label(&route.namespace);
            let link: AnyElement = if parent.is_gateway() {
                mono_link(
                    format!("parent-{ix}"),
                    parent_label,
                    self.gateway_ref(&parent.namespace, &parent.name, target, cx),
                    colors,
                )
                .into_any_element()
            } else {
                mono_at(ix, parent_label, colors.text).into_any_element()
            };
            let mut card = card(colors).child(
                h_flex()
                    .gap(u(6.0))
                    .child(
                        Icon::new(tone_icon(tone))
                            .size(12.0)
                            .color(tone_color(tone, colors)),
                    )
                    .child(link)
                    .child(div().flex_1())
                    .child(state(
                        SharedString::from(format!("parent-{ix}-state")),
                        label,
                        tone,
                        colors,
                    )),
            );
            if let Some(status) = status {
                if !status.controller.is_empty() {
                    card = card.child(
                        text_at(
                            ix,
                            format!("controller {}", status.controller),
                            colors.text_dim,
                        )
                        .text_size(u(11.5)),
                    );
                }
                for (cx_ix, condition) in status
                    .conditions
                    .iter()
                    .filter(|c| c.is_false())
                    .enumerate()
                {
                    let message = if condition.message.is_empty() {
                        condition.reason.clone()
                    } else {
                        format!("{}: {}", condition.reason, condition.message)
                    };
                    card = card.child(
                        text_at(
                            ix * 100 + cx_ix,
                            format!("{} · {message}", condition.kind),
                            colors.red,
                        )
                        .text_size(u(11.5))
                        .wrapped(),
                    );
                }
            } else {
                card = card.child(
                    text_at(
                        ix,
                        "No controller has reported on this parent yet.",
                        colors.text_dim,
                    )
                    .text_size(u(11.5)),
                );
            }
            parents = parents.child(card);
        }
        out.push(
            section(format!("Parents · {}", route.parents.len()), colors)
                .child(parents)
                .into_any_element(),
        );

        let mut rules = list();
        for (ix, rule) in route.rules.iter().enumerate() {
            let title = if rule.name.is_empty() {
                format!("Rule {}", ix + 1)
            } else {
                rule.name.clone()
            };
            let mut card =
                card(colors).child(text_at(ix, title, colors.text_dim).text_size(u(11.5)));
            for (mx, m) in rule.matches.iter().enumerate() {
                card = card.child(mono_at(ix * 100 + mx, m.clone(), colors.text));
            }
            if rule.matches.is_empty() && route.kind != "TCPRoute" && route.kind != "UDPRoute" {
                card = card.child(text_at(ix, "matches everything", colors.text_muted));
            }
            if !rule.filters.is_empty() {
                card = card.child(
                    text_at(
                        ix,
                        format!("filters {}", rule.filters.join(", ")),
                        colors.text_dim,
                    )
                    .text_size(u(11.5)),
                );
            }
            let total: i64 = rule.backends.iter().map(|b| b.weight).sum();
            for (bx, backend) in rule.backends.iter().enumerate() {
                let label = backend.label(&route.namespace);
                let link: AnyElement = if backend.is_service() {
                    let reference = ResourceRef::object(
                        target.cluster.clone(),
                        Gvr::new("", "v1", "services"),
                        Some(backend.namespace.clone()),
                        backend.name.clone(),
                    );
                    mono_link(format!("backend-{ix}-{bx}"), label, reference, colors)
                        .into_any_element()
                } else {
                    mono_at(ix * 100 + bx, label, colors.text).into_any_element()
                };
                let mut row = h_flex()
                    .gap(u(6.0))
                    .child(
                        Icon::new(IconName::ArrowRight)
                            .size(11.0)
                            .color(colors.text_dim),
                    )
                    .child(link)
                    .child(div().flex_1());
                if rule.backends.len() > 1 && total > 0 {
                    row = row.child(
                        text_at(
                            ix * 100 + bx,
                            format!(
                                "weight {} ({}%)",
                                backend.weight,
                                backend.weight * 100 / total
                            ),
                            colors.text_dim,
                        )
                        .text_size(u(11.5)),
                    );
                }
                card = card.child(row);
            }
            if rule.backends.is_empty() {
                card = card.child(text_at(ix, "no backends", colors.yellow));
            }
            rules = rules.child(card);
        }
        if route.rules.is_empty() {
            rules = rules.child(empty("No rules.", colors));
        }
        out.push(
            section(format!("Rules · {}", route.rules.len()), colors)
                .child(rules)
                .into_any_element(),
        );
        out
    }

    /// A Service's routes and EndpointSlices.
    fn render_service_routes(&self, target: &Target, colors: &Colors, cx: &App) -> Vec<AnyElement> {
        let mut out = Vec::new();
        let ns = target.namespace.clone().unwrap_or_default();
        if gateway::ROUTE_KINDS
            .iter()
            .any(|(kind, _)| self.has_named(&route_store(kind)))
        {
            let found = self.derived(
                "service-routes",
                &[ROUTES, ROUTES_FROM, "grants"],
                cx,
                |this| {
                    let grants = this.named("grants", cx);
                    let mut routes = this.routes_in(ROUTES, cx);
                    routes.extend(
                        this.routes_in(ROUTES_FROM, cx)
                            .into_iter()
                            .filter(|(_, route)| route.namespace != ns),
                    );
                    sorted(
                        routes
                            .into_iter()
                            .filter(|(_, route)| {
                                !route.backends_to(&ns, &target.name).is_empty()
                                    && (route.namespace == ns
                                        || grants.iter().any(|g| {
                                            gateway::grant_allows(
                                                g,
                                                &route.kind,
                                                &route.namespace,
                                                &target.name,
                                            )
                                        }))
                            })
                            .collect(),
                    )
                },
            );
            let mut body = list().gap(u(5.0));
            if found.is_empty() {
                body = body.child(empty("No route sends traffic to this Service.", colors));
            }
            for (ix, (route_object, route)) in found.iter().take(LIMIT).enumerate() {
                let (_, tone) = route.state();
                let backends = route.backends_to(&ns, &target.name);
                let total_weights: Vec<String> = backends
                    .iter()
                    .map(|b| {
                        let mut s = match b.port {
                            Some(port) => format!("port {port}"),
                            None => "no port".into(),
                        };
                        if b.weight != 1 {
                            s.push_str(&format!(" · weight {}", b.weight));
                        }
                        s
                    })
                    .collect();
                let label = if route.namespace == ns {
                    name(route_object).to_string()
                } else {
                    format!("{}/{}", route.namespace, name(route_object))
                };
                let gateways: Vec<String> = route
                    .parents
                    .iter()
                    .filter(|p| p.is_gateway())
                    .map(|p| {
                        if p.namespace == route.namespace {
                            p.name.clone()
                        } else {
                            format!("{}/{}", p.namespace, p.name)
                        }
                    })
                    .fold(Vec::new(), |mut acc, g| {
                        if !acc.contains(&g) {
                            acc.push(g);
                        }
                        acc
                    });
                let mut second = Vec::new();
                if !route.hostnames.is_empty() {
                    second.push(route.hostnames.join(", "));
                }
                if !gateways.is_empty() {
                    second.push(format!("via {}", gateways.join(", ")));
                }
                body = body.child(
                    v_flex()
                        .child(
                            h_flex()
                                .gap(u(7.0))
                                .child(
                                    Icon::new(tone_icon(tone))
                                        .size(12.0)
                                        .color(tone_color(tone, colors)),
                                )
                                .child(text_at(ix, route.kind.clone(), colors.text_dim).flex_none())
                                .child(mono_link(
                                    format!("svc-route-{ix}"),
                                    label,
                                    self.route_ref(route_object, target, cx),
                                    colors,
                                ))
                                .child(div().flex_1())
                                .child(
                                    text_at(ix, total_weights.join(", "), colors.text_dim)
                                        .text_size(u(11.5)),
                                ),
                        )
                        .when(!second.is_empty(), |this| {
                            this.child(
                                text_at(ix, second.join(" · "), colors.text_dim)
                                    .text_size(u(11.5))
                                    .pl(u(19.0)),
                            )
                        }),
                );
            }
            out.push(
                section(format!("Routes · {}", found.len()), colors)
                    .child(body)
                    .into_any_element(),
            );
        }
        if self.has_named("endpointslices") {
            let slices = self.named("endpointslices", cx);
            let mut body = list().gap(u(4.0));
            if slices.is_empty() {
                body = body.child(empty("No EndpointSlices.", colors));
            }
            for (ix, slice) in slices.iter().take(LIMIT).enumerate() {
                let (ready, total) = endpoint_counts(slice);
                let reference = self.reference(
                    target,
                    "discovery.k8s.io",
                    "endpointslices",
                    target.namespace.clone(),
                    name(slice),
                    cx,
                );
                let facts = format!(
                    "{} · {ready}/{total} ready · {}",
                    str_at(slice, "/addressType"),
                    endpoint_ports(slice).join(", ")
                );
                body = body.child(
                    h_flex()
                        .gap(u(7.0))
                        .child(StatusDot::new(if ready < total {
                            colors.yellow
                        } else {
                            colors.green
                        }))
                        .child(mono_link(
                            format!("svc-slice-{ix}"),
                            name(slice).to_string(),
                            reference,
                            colors,
                        ))
                        .child(text_at(ix, facts, colors.text_dim).text_size(u(11.5))),
                );
            }
            out.push(
                section(format!("EndpointSlices · {}", slices.len()), colors)
                    .child(body)
                    .into_any_element(),
            );
        }
        out
    }
}

/// An EndpointSlice: its Service, address type and ports, and each endpoint.
fn render_endpoint_slice(object: &Value, target: &Target, colors: &Colors) -> Vec<AnyElement> {
    let service = str_at(object, "/metadata/labels/kubernetes.io~1service-name");
    let mut facts = v_flex().gap(u(4.0));
    if !service.is_empty() {
        let reference = ResourceRef::object(
            target.cluster.clone(),
            Gvr::new("", "v1", "services"),
            target.namespace.clone(),
            service.to_string(),
        );
        facts = facts.child(kv_row(
            "Service",
            80.0,
            mono_link("slice-service", service.to_string(), reference, colors),
            colors,
        ));
    }
    facts = facts.child(kv_row(
        "Address type",
        80.0,
        text(str_at(object, "/addressType").to_string(), colors.text),
        colors,
    ));
    facts = facts.child(kv_row(
        "Ports",
        80.0,
        text(endpoint_ports(object).join(", "), colors.text),
        colors,
    ));
    let managed = str_at(
        object,
        "/metadata/labels/endpointslice.kubernetes.io~1managed-by",
    );
    if !managed.is_empty() {
        facts = facts.child(kv_row(
            "Managed by",
            80.0,
            text(managed.to_string(), colors.text_muted),
            colors,
        ));
    }
    let mut out = vec![
        section("Endpoint slice", colors)
            .child(facts)
            .into_any_element(),
    ];

    let endpoints = array_at(object, "/endpoints");
    let (ready, total) = endpoint_counts(object);
    let mut body = list().gap(u(4.0));
    for (ix, endpoint) in endpoints.iter().take(LIMIT * 2).enumerate() {
        let flag = |key: &str| {
            endpoint
                .pointer(&format!("/conditions/{key}"))
                .and_then(Value::as_bool)
        };
        let (label, tone) = match (flag("ready"), flag("terminating")) {
            (_, Some(true)) => ("terminating", Tone::Warning),
            (Some(false), _) => ("not ready", Tone::Bad),
            _ => ("ready", Tone::Good),
        };
        let addresses: Vec<&str> = array_at(endpoint, "/addresses")
            .iter()
            .filter_map(Value::as_str)
            .collect();
        let mut row = h_flex()
            .gap(u(7.0))
            .child(StatusDot::new(tone_color(tone, colors)))
            .child(mono_at(ix, addresses.join(", "), colors.text));
        if let Some(target_ref) = endpoint.get("targetRef")
            && str_at(target_ref, "/kind") == "Pod"
        {
            let pod = str_at(target_ref, "/name").to_string();
            let ns = match str_at(target_ref, "/namespace") {
                "" => target.namespace.clone(),
                ns => Some(ns.to_string()),
            };
            let reference = ResourceRef::object(
                target.cluster.clone(),
                Gvr::new("", "v1", "pods"),
                ns,
                pod.clone(),
            );
            row = row.child(mono_link(
                format!("endpoint-pod-{ix}"),
                pod,
                reference,
                colors,
            ));
        }
        row = row.child(div().flex_1());
        let node = str_at(endpoint, "/nodeName");
        let mut tail = vec![label.to_string()];
        if !node.is_empty() {
            tail.push(node.to_string());
        }
        let zone = str_at(endpoint, "/zone");
        if !zone.is_empty() {
            tail.push(zone.to_string());
        }
        row = row.child(text_at(ix, tail.join(" · "), tone_color(tone, colors)).text_size(u(11.5)));
        body = body.child(row);
    }
    if endpoints.is_empty() {
        body = body.child(empty("No endpoints.", colors));
    } else if endpoints.len() > LIMIT * 2 {
        body = body.child(subtitle(
            format!("… {} more", endpoints.len() - LIMIT * 2),
            colors,
        ));
    }
    out.push(
        section(format!("Endpoints · {ready}/{total} ready"), colors)
            .child(body)
            .into_any_element(),
    );
    out
}
