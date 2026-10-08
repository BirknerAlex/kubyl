//! Device resources (DRA) in the details: a claim's requests, allocation, reserved-for pods and
//! device status; a slice's devices with the claims holding them; a class's selectors and
//! config; a template's requests; a pod's "Resource claims" and a node's "Devices".

use std::collections::HashMap;
use std::rc::Rc;
use std::sync::Arc;

use gpui::{AnyElement, Context, IntoElement, SharedString, div, prelude::*};
use kubyl_core::{Gvr, ResourceRef, Tone};
use kubyl_resources::columns::pod_status;
use kubyl_resources::dra::{self, Claim, Class, Slice};
use kubyl_resources::format::{
    array_at, human_duration, name, namespace, seconds_since, timestamp,
};
use kubyl_ui::{Chip, Colors, Icon, IconName, fonts, h_flex, tone_color, u, v_flex};
use serde_json::Value;

use super::parts::{
    Wrapped, card, cel, empty, kv_row, list, mono_at, mono_link, state, subtitle, text, text_at,
};
use super::{DetailsContent, Target, chips, section};

/// Devices listed per slice or node before "… N more".
const DEVICE_LIMIT: usize = 24;
const CLAIMS: (&str, &str) = (dra::GROUP, "resourceclaims");
const SLICES: (&str, &str) = (dra::GROUP, "resourceslices");

impl DetailsContent {
    pub(super) fn load_devices(&mut self, target: &Target, object: &Value, cx: &mut Context<Self>) {
        let ns = target.namespace.clone();
        match target.kind.as_str() {
            "Pod" => {
                if !array_at(object, "/spec/resourceClaims").is_empty() {
                    self.watch_named("claims", target, CLAIMS, ns, None, cx);
                }
            }
            "Node" => {
                // The node's own slices (`spec.nodeName`); the claims only once it has some.
                let fields = format!("spec.nodeName={}", target.name);
                if self.watch_named("slices", target, SLICES, None, Some(fields), cx)
                    && let Some(slices) = self.related.named.get("slices").cloned()
                {
                    let observer =
                        cx.observe(slices.entity(), |this, _, cx| this.node_slices_changed(cx));
                    self.related._observers.push(observer);
                    self.node_slices_changed(cx);
                }
            }
            "ResourceClaim" => {
                self.watch_named("slices", target, SLICES, None, None, cx);
                if let Some(ns) = ns {
                    let pods = kubyl_resources::StoreKey::new(
                        target.cluster.clone(),
                        Gvr::new("", "v1", "pods"),
                        Some(ns),
                    );
                    let handle = self.acquire(pods, cx);
                    self.related.named.insert("pods".into(), handle.clone());
                    self.related.pods = Some(handle);
                }
            }
            "ResourceSlice" | "DeviceClass" => {
                self.watch_named("claims", target, CLAIMS, None, None, cx);
            }
            _ => {}
        }
    }

    /// Watches the claims once the node's slices loaded with at least one.
    fn node_slices_changed(&mut self, cx: &mut Context<Self>) {
        if self.has_named("claims") {
            return;
        }
        let Some(target) = self.target.clone() else {
            return;
        };
        let has_slices = self
            .related
            .named
            .get("slices")
            .is_some_and(|s| !s.read(cx).objects().is_empty());
        if has_slices && self.watch_named("claims", &target, CLAIMS, None, None, cx) {
            cx.notify();
        }
    }

    /// The claims holding each device, once the claims loaded.
    pub(super) fn holders(&self, cx: &gpui::App) -> Option<Rc<dra::DeviceHolders>> {
        if !self.named_ready("claims", cx) {
            return None;
        }
        Some(self.derived("holders", &["claims"], cx, |this| {
            let claims = this.related.named.get("claims");
            dra::DeviceHolders::index(
                claims
                    .into_iter()
                    .flat_map(|c| c.read(cx).objects().values().map(|o| &**o)),
            )
        }))
    }

    pub(super) fn render_devices(
        &mut self,
        object: &Value,
        target: &Target,
        colors: &Colors,
        cx: &mut Context<Self>,
    ) -> Vec<AnyElement> {
        match target.kind.as_str() {
            "ResourceClaim" => self.render_claim(object, target, colors, cx),
            "ResourceClaimTemplate" => {
                let requests =
                    dra::requests(object.pointer("/spec/spec/devices").unwrap_or(&Value::Null));
                vec![self.requests_section(&requests, target, colors, cx)]
            }
            "DeviceClass" => self.render_device_class(object, target, colors, cx),
            "ResourceSlice" => self.render_slice(object, target, colors, cx),
            "Pod" => self
                .render_pod_claims(object, target, colors, cx)
                .into_iter()
                .collect(),
            "Node" => self
                .render_node_devices(target, colors, cx)
                .into_iter()
                .collect(),
            _ => Vec::new(),
        }
    }

    fn class_link(
        &self,
        id: String,
        class: &str,
        target: &Target,
        colors: &Colors,
        cx: &gpui::App,
    ) -> impl IntoElement {
        let reference = self.reference(target, dra::GROUP, "deviceclasses", None, class, cx);
        mono_link(id, class.to_string(), reference, colors)
    }

    fn requests_section(
        &self,
        requests: &[dra::Request],
        target: &Target,
        colors: &Colors,
        cx: &gpui::App,
    ) -> AnyElement {
        let mut body = list();
        if requests.is_empty() {
            body = body.child(empty("No device requests.", colors));
        }
        for (ix, request) in requests.iter().enumerate() {
            body = body.child(self.request_card(ix, request, target, colors, cx));
        }
        section(format!("Requests · {}", requests.len()), colors)
            .child(body)
            .into_any_element()
    }

    fn request_card(
        &self,
        ix: usize,
        request: &dra::Request,
        target: &Target,
        colors: &Colors,
        cx: &gpui::App,
    ) -> impl IntoElement {
        let mut head = h_flex()
            .gap(u(6.0))
            .child(
                text_at(ix, request.name.clone(), colors.text)
                    .font_weight(gpui::FontWeight::MEDIUM),
            )
            .child(Chip::new(request.amount()).selectable_as(("amount", ix as u64)))
            .child(div().flex_1());
        if !request.device_class.is_empty() {
            head = head.child(self.class_link(
                format!("request-class-{ix}"),
                &request.device_class,
                target,
                colors,
                cx,
            ));
        }
        let mut facts = Vec::new();
        if request.alternatives.is_empty() {
            facts.push(request.mode.clone());
        }
        if request.selectors.is_empty() {
            facts.push("no selectors".into());
        }
        if request.admin_access {
            facts.push("admin access".into());
        }
        if !request.tolerations.is_empty() {
            facts.push(format!("tolerates {}", request.tolerations.join(", ")));
        }
        let mut body = card(colors)
            .child(head)
            .child(text_at(ix, facts.join(" · "), colors.text_dim).text_size(u(11.5)));
        for (sx, selector) in request.selectors.iter().enumerate() {
            body = body.child(cel(
                SharedString::from(format!("request-{ix}-selector-{sx}")),
                selector,
                colors,
            ));
        }
        for (ax, alternative) in request.alternatives.iter().enumerate() {
            body = body.child(
                h_flex()
                    .gap(u(6.0))
                    .text_size(u(11.5))
                    .child(mono_at(
                        ix * 100 + ax,
                        format!("{}.", ax + 1),
                        colors.text_dim,
                    ))
                    .child(mono_at(
                        ix * 100 + ax,
                        alternative.name.clone(),
                        colors.text,
                    ))
                    .child(text_at(
                        ix * 100 + ax,
                        alternative.amount(),
                        colors.text_dim,
                    ))
                    .child(div().flex_1())
                    .child(self.class_link(
                        format!("request-{ix}-alt-{ax}"),
                        &alternative.device_class,
                        target,
                        colors,
                        cx,
                    )),
            );
        }
        body
    }

    /// The slice that holds `device` (once the slices loaded).
    fn slice_of(&self, device: &dra::AllocatedDevice, cx: &gpui::App) -> Option<String> {
        let index = self.derived("slice-of", &["slices"], cx, |this| {
            let mut index: HashMap<(String, String, String), String> = HashMap::new();
            for slice in this.named("slices", cx) {
                let parsed = Slice::parse(&slice);
                for d in parsed.devices {
                    index
                        .entry((parsed.driver.clone(), parsed.pool.clone(), d.name))
                        .or_insert_with(|| name(&slice).to_string());
                }
            }
            index
        });
        index
            .get(&(
                device.driver.clone(),
                device.pool.clone(),
                device.device.clone(),
            ))
            .cloned()
    }

    /// Links to the claims holding a device.
    fn holder_links(
        &self,
        id: &str,
        claims: &[String],
        target: &Target,
        colors: &Colors,
        cx: &gpui::App,
    ) -> impl IntoElement {
        let mut row = h_flex().gap(u(5.0)).flex_none();
        for (hx, claim) in claims.iter().enumerate() {
            row = row.child(mono_link(
                format!("{id}-{hx}"),
                claim.clone(),
                self.claim_ref(claim, target, cx),
                colors,
            ));
        }
        row
    }

    fn render_claim(
        &self,
        object: &Value,
        target: &Target,
        colors: &Colors,
        cx: &mut Context<Self>,
    ) -> Vec<AnyElement> {
        let claim = Claim::parse(object);
        let now = jiff::Timestamp::now();
        let mut out = vec![self.requests_section(&claim.requests, target, colors, cx)];

        // Allocation: node, driver and pool, the devices with their slice and health.
        let mut body = v_flex().gap(u(4.0));
        match &claim.allocation {
            None => body = body.child(empty("Not allocated yet: the scheduler allocates the claim for the first pod that uses it.", colors).wrapped()),
            Some(allocation) => {
                if let Some(node) = &allocation.node {
                    let reference = ResourceRef::object(target.cluster.clone(), Gvr::new("", "v1", "nodes"), None, node.clone());
                    body = body.child(kv_row("Node", 70.0, mono_link("claim-node", node.clone(), reference, colors), colors));
                } else if !allocation.node_selector.is_empty() {
                    body = body.child(kv_row("Nodes", 70.0, text(allocation.node_selector.join(" or "), colors.text), colors));
                } else {
                    body = body.child(kv_row("Nodes", 70.0, text("any node", colors.text), colors));
                }
                let drivers = distinct(allocation.devices.iter().map(|d| d.driver.as_str()));
                if !drivers.is_empty() {
                    body = body.child(kv_row("Driver", 70.0, text(drivers.join(", "), colors.text).font_family(fonts::MONO).text_size(u(11.5)), colors));
                }
                let pools = distinct(allocation.devices.iter().map(|d| d.pool.as_str()));
                if !pools.is_empty() {
                    body = body.child(kv_row("Pool", 70.0, text(pools.join(", "), colors.text).font_family(fonts::MONO).text_size(u(11.5)), colors));
                }
                if let Some(time) = allocation.time.as_deref().and_then(timestamp) {
                    body = body.child(kv_row("Allocated", 70.0, text(format!("{} ago", human_duration(seconds_since(time, now))), colors.text), colors));
                }
                let pods = self.derived("pod-claims", &["pods"], cx, |this| {
                    dra::PodClaims::index(this.related.named.get("pods").into_iter().flat_map(|p| p.read(cx).objects().values().map(|o| &**o)))
                });
                let health = dra::claim_health(object, &pods);
                body = body.child(subtitle(format!("Devices · {}", allocation.devices.len()), colors));
                for (ix, device) in allocation.devices.iter().enumerate() {
                    let status = claim.device_status(device);
                    let ready = status.and_then(|s| s.conditions.iter().find(|c| c.kind == "Ready"));
                    let (label, tone) = match ready {
                        Some(c) if c.is_true() => ("Ready".to_string(), Tone::Good),
                        Some(c) if c.is_false() => (if c.reason.is_empty() { "Not ready".into() } else { c.reason.clone() }, Tone::Bad),
                        _ => match health.label() {
                            Some((_, Tone::Good)) => ("Healthy".into(), Tone::Good),
                            Some((_, Tone::Bad)) => ("Unhealthy".into(), Tone::Bad),
                            _ => (String::new(), Tone::Neutral),
                        },
                    };
                    let mut row = h_flex()
                        .gap(u(7.0))
                        .text_size(u(12.0))
                        .child(Icon::new(IconName::Cpu).size(12.0).color(tone_color(if label.is_empty() { Tone::Neutral } else { tone }, colors)))
                        .child(mono_at(ix, device.device.clone(), colors.text).flex_none())
                        .child(text_at(ix, format!("for {}", device.request), colors.text_dim).flex_none());
                    if let Some(slice) = self.slice_of(device, cx) {
                        let reference = self.reference(target, dra::GROUP, "resourceslices", None, slice.clone(), cx);
                        row = row
                            .child(text_at(ix, "in", colors.text_dim).flex_none())
                            .child(mono_link(format!("claim-slice-{ix}"), slice, reference, colors));
                    }
                    row = row.child(div().flex_1());
                    if !label.is_empty() {
                        row = row.child(text_at(ix, label, tone_color(tone, colors)).text_size(u(11.5)).flex_none());
                    }
                    body = body.child(row);
                }
            }
        }
        out.push(section("Allocation", colors).child(body).into_any_element());

        // Reserved for: the pods (or other consumers) using it.
        let mut reserved = list().gap(u(4.0));
        if claim.reserved_for.is_empty() {
            reserved = reserved.child(empty("Not in use.", colors));
        }
        for (ix, consumer) in claim.reserved_for.iter().enumerate() {
            let mut row = h_flex().gap(u(7.0));
            if consumer.resource == "pods" && consumer.api_group.is_empty() {
                let pod = self.related.pods.as_ref().and_then(|p| {
                    p.read(cx)
                        .get(&kubyl_resources::object_key(
                            target.namespace.as_deref(),
                            &consumer.name,
                        ))
                        .cloned()
                });
                let (label, tone) = match &pod {
                    Some(pod) => {
                        let status = pod_status(pod);
                        let tone = status.tone();
                        (status.reason, tone)
                    }
                    None => (String::new(), Tone::Neutral),
                };
                let reference = ResourceRef::object(
                    target.cluster.clone(),
                    Gvr::new("", "v1", "pods"),
                    target.namespace.clone(),
                    consumer.name.clone(),
                );
                row = row
                    .child(kubyl_ui::StatusDot::new(tone_color(tone, colors)))
                    .child(text_at(ix, "Pod", colors.text_dim).flex_none())
                    .child(mono_link(
                        format!("reserved-{ix}"),
                        consumer.name.clone(),
                        reference,
                        colors,
                    ))
                    .child(div().flex_1())
                    .child(text_at(ix, label, tone_color(tone, colors)).flex_none());
            } else {
                row = row
                    .child(text_at(ix, consumer.resource.clone(), colors.text_dim))
                    .child(mono_at(ix, consumer.name.clone(), colors.text));
            }
            reserved = reserved.child(row);
        }
        out.push(
            section(
                format!("Reserved for · {}", claim.reserved_for.len()),
                colors,
            )
            .child(reserved)
            .into_any_element(),
        );

        // What the drivers report per device.
        if !claim.devices.is_empty() {
            let mut body = list();
            for (ix, device) in claim.devices.iter().enumerate() {
                let mut head = h_flex()
                    .gap(u(6.0))
                    .child(mono_at(ix, device.device.clone(), colors.text).flex_none())
                    .child(div().flex_1());
                for (cx_ix, condition) in device.conditions.iter().enumerate() {
                    let healthy = condition.is_true();
                    head = head.child(
                        h_flex()
                            .gap(u(4.0))
                            .child(
                                Icon::new(if healthy {
                                    IconName::CircleCheck
                                } else {
                                    IconName::CircleX
                                })
                                .size(12.0)
                                .color(if healthy {
                                    colors.green
                                } else {
                                    colors.red
                                }),
                            )
                            .child(text_at(
                                ix * 100 + cx_ix,
                                condition.kind.clone(),
                                colors.text,
                            )),
                    );
                }
                let mut card = card(colors).child(head);
                if !device.data.is_empty() {
                    card = card.child(chips(device.data.clone(), true));
                }
                if !device.network.is_empty() {
                    card = card.child(
                        text_at(ix, device.network.join(" · "), colors.text_dim).text_size(u(11.5)),
                    );
                }
                body = body.child(card);
            }
            out.push(
                section("Device status", colors)
                    .child(body)
                    .into_any_element(),
            );
        }
        out
    }

    fn render_device_class(
        &self,
        object: &Value,
        target: &Target,
        colors: &Colors,
        cx: &gpui::App,
    ) -> Vec<AnyElement> {
        let class = Class::parse(object);
        let mut facts = v_flex().gap(u(4.0));
        let drivers = class.drivers();
        facts = facts.child(kv_row(
            "Drivers",
            110.0,
            text(
                if drivers.is_empty() {
                    "any (no driver selector)".into()
                } else {
                    drivers.join(", ")
                },
                colors.text,
            ),
            colors,
        ));
        facts = facts.child(kv_row(
            "Extended resource",
            110.0,
            text(
                class
                    .extended_resource_name
                    .clone()
                    .unwrap_or_else(|| "none".into()),
                colors.text,
            ),
            colors,
        ));
        let mut out = vec![
            section("Device class", colors)
                .child(facts)
                .into_any_element(),
        ];

        let mut selectors = list();
        if class.selectors.is_empty() {
            selectors = selectors.child(empty(
                "No selectors: every device of the cluster matches.",
                colors,
            ));
        }
        for (ix, selector) in class.selectors.iter().enumerate() {
            selectors = selectors.child(cel(
                SharedString::from(format!("class-selector-{ix}")),
                selector,
                colors,
            ));
        }
        out.push(
            section(format!("Selectors · {}", class.selectors.len()), colors)
                .child(selectors)
                .into_any_element(),
        );

        if !class.config.is_empty() {
            let mut config = list();
            for (ix, (driver, parameters)) in class.config.iter().enumerate() {
                config = config.child(
                    card(colors)
                        .child(
                            text_at(ix, format!("opaque · {driver}"), colors.text_dim)
                                .text_size(u(11.5)),
                        )
                        .child(cel(
                            SharedString::from(format!("class-config-{ix}")),
                            parameters,
                            colors,
                        )),
                );
            }
            out.push(section("Config", colors).child(config).into_any_element());
        }

        if self.has_named("claims") {
            let me = name(object).to_string();
            let claims = self.derived("class-claims", &["claims"], cx, |this| {
                this.named("claims", cx)
                    .into_iter()
                    .filter(|c| Claim::parse(c).device_classes().contains(&me))
                    .collect::<Vec<Arc<Value>>>()
            });
            out.push(self.claim_list("Claims", &claims, target, colors, cx));
        }
        out
    }

    /// Claims as links with their state.
    fn claim_list(
        &self,
        title: &str,
        claims: &[Arc<Value>],
        target: &Target,
        colors: &Colors,
        cx: &gpui::App,
    ) -> AnyElement {
        let mut body = list().gap(u(4.0));
        if claims.is_empty() {
            body = body.child(empty("None.", colors));
        }
        for (ix, claim) in claims.iter().take(DEVICE_LIMIT).enumerate() {
            let state_label = Claim::parse(claim).state();
            let tone = dra::claim_tone(&state_label);
            let ns = namespace(claim).map(String::from);
            let label = match &ns {
                Some(ns) => format!("{ns}/{}", name(claim)),
                None => name(claim).to_string(),
            };
            let reference =
                self.reference(target, dra::GROUP, "resourceclaims", ns, name(claim), cx);
            body = body.child(
                h_flex()
                    .gap(u(7.0))
                    .child(kubyl_ui::StatusDot::new(tone_color(tone, colors)))
                    .child(mono_link(format!("claim-{ix}"), label, reference, colors))
                    .child(div().flex_1())
                    .child(text_at(ix, state_label, tone_color(tone, colors)).flex_none()),
            );
        }
        if claims.len() > DEVICE_LIMIT {
            body = body.child(empty(
                format!("… {} more", claims.len() - DEVICE_LIMIT),
                colors,
            ));
        }
        section(format!("{title} · {}", claims.len()), colors)
            .child(body)
            .into_any_element()
    }

    /// A link to the claim `namespace/name`.
    fn claim_ref(&self, label: &str, target: &Target, cx: &gpui::App) -> ResourceRef {
        let (ns, claim) = match label.split_once('/') {
            Some((ns, claim)) => (Some(ns.to_string()), claim),
            None => (None, label),
        };
        self.reference(target, dra::GROUP, "resourceclaims", ns, claim, cx)
    }

    fn render_slice(
        &self,
        object: &Value,
        target: &Target,
        colors: &Colors,
        cx: &gpui::App,
    ) -> Vec<AnyElement> {
        let slice = Slice::parse(object);
        let holders = self.holders(cx);
        let held = holders
            .as_deref()
            .map(|h| dra::allocations(&slice, h))
            .unwrap_or_default();
        let mut facts = v_flex().gap(u(4.0));
        facts = facts.child(kv_row(
            "Driver",
            70.0,
            text(slice.driver.clone(), colors.text)
                .font_family(fonts::MONO)
                .text_size(u(11.5)),
            colors,
        ));
        facts = facts.child(kv_row(
            "Pool",
            70.0,
            text(
                format!(
                    "{} · generation {} · {} slice{}",
                    slice.pool,
                    slice.generation,
                    slice.slice_count,
                    if slice.slice_count == 1 { "" } else { "s" }
                ),
                colors.text,
            ),
            colors,
        ));
        let node = match &slice.node {
            Some(node) => {
                let reference = ResourceRef::object(
                    target.cluster.clone(),
                    Gvr::new("", "v1", "nodes"),
                    None,
                    node.clone(),
                );
                mono_link("slice-node", node.clone(), reference, colors).into_any_element()
            }
            None => text(slice.node_label(), colors.text).into_any_element(),
        };
        facts = facts.child(kv_row("Node", 70.0, node, colors));
        if holders.is_some() {
            facts = facts.child(kv_row(
                "Allocated",
                70.0,
                text(
                    format!("{} of {} devices", held.len(), slice.devices.len()),
                    colors.text,
                ),
                colors,
            ));
        }
        let mut out = vec![section("Slice", colors).child(facts).into_any_element()];

        let mut devices = list();
        for (ix, device) in slice.devices.iter().take(DEVICE_LIMIT).enumerate() {
            let holder = held
                .iter()
                .find(|(d, _)| d == &device.name)
                .map(|(_, claims)| claims.as_slice());
            let mut head = h_flex()
                .gap(u(6.0))
                .child(
                    Icon::new(IconName::Cpu)
                        .size(12.0)
                        .color(if holder.is_some() {
                            colors.green
                        } else {
                            colors.text_dim
                        }),
                )
                .child(mono_at(ix, device.name.clone(), colors.text))
                .child(div().flex_1());
            head = match holder {
                Some(claims) => head
                    .child(
                        text_at(
                            ix,
                            if claims.len() == 1 { "claim" } else { "claims" },
                            colors.text_dim,
                        )
                        .flex_none(),
                    )
                    .child(self.holder_links(
                        &format!("device-claim-{ix}"),
                        claims,
                        target,
                        colors,
                        cx,
                    )),
                None if holders.is_some() => {
                    head.child(text_at(ix, "free", colors.text_dim).flex_none())
                }
                None => head,
            };
            let mut body = card(colors).child(head);
            if !device.attributes.is_empty() {
                body = body.child(chips(
                    device
                        .attributes
                        .iter()
                        .map(|(k, v)| format!("{k}={v}"))
                        .collect(),
                    true,
                ));
            }
            if !device.capacity.is_empty() {
                let capacity: Vec<String> = device
                    .capacity
                    .iter()
                    .map(|(k, v)| format!("{k} {v}"))
                    .collect();
                body = body.child(
                    text_at(
                        ix,
                        format!("capacity {}", capacity.join(" · ")),
                        colors.text_dim,
                    )
                    .text_size(u(11.5)),
                );
            }
            for (tx, taint) in device.taints.iter().enumerate() {
                body = body.child(
                    h_flex()
                        .gap(u(4.0))
                        .child(
                            Icon::new(IconName::TriangleAlert)
                                .size(11.0)
                                .color(colors.yellow),
                        )
                        .child(
                            text_at(
                                ix * 100 + tx,
                                format!("taint {}", taint.label()),
                                colors.yellow,
                            )
                            .text_size(u(11.5)),
                        ),
                );
            }
            if let Some(node) = &device.node {
                body = body
                    .child(text_at(ix, format!("node {node}"), colors.text_dim).text_size(u(11.5)));
            }
            devices = devices.child(body);
        }
        if slice.devices.len() > DEVICE_LIMIT {
            devices = devices.child(empty(
                format!("… {} more", slice.devices.len() - DEVICE_LIMIT),
                colors,
            ));
        }
        if slice.devices.is_empty() {
            devices = devices.child(empty("No devices.", colors));
        }
        out.push(
            section(format!("Devices · {}", slice.devices.len()), colors)
                .child(devices)
                .into_any_element(),
        );
        out
    }

    /// A pod's "Resource claims": each claim with its state, devices and health.
    fn render_pod_claims(
        &self,
        pod: &Value,
        target: &Target,
        colors: &Colors,
        cx: &gpui::App,
    ) -> Option<AnyElement> {
        let claims = dra::pod_claims(pod);
        if claims.is_empty() {
            return None;
        }
        let mut body = list();
        for (ix, pod_claim) in claims.iter().enumerate() {
            let object = pod_claim
                .claim
                .as_deref()
                .and_then(|c| self.named_get("claims", target.namespace.as_deref(), c, cx));
            let parsed = object.as_deref().map(Claim::parse);
            let mut head = h_flex()
                .gap(u(6.0))
                .child(
                    text_at(ix, pod_claim.name.clone(), colors.text)
                        .font_weight(gpui::FontWeight::MEDIUM),
                )
                .child(text_at(ix, "→", colors.text_dim));
            head = match &pod_claim.claim {
                Some(claim) => head.child(mono_link(
                    format!("pod-claim-{ix}"),
                    claim.clone(),
                    self.reference(
                        target,
                        dra::GROUP,
                        "resourceclaims",
                        target.namespace.clone(),
                        claim.clone(),
                        cx,
                    ),
                    colors,
                )),
                None => head.child(text_at(ix, "not created yet", colors.yellow)),
            };
            head = head.child(div().flex_1());
            if let Some(parsed) = &parsed {
                let label = parsed.state();
                let tone = dra::claim_tone(&label);
                head = head.child(state(
                    SharedString::from(format!("pod-claim-{ix}-state")),
                    label,
                    tone,
                    colors,
                ));
            }
            let mut facts = Vec::new();
            if let Some(template) = &pod_claim.template {
                facts.push(format!("from template {template}"));
            }
            if !pod_claim.containers.is_empty() {
                facts.push(format!(
                    "container{} {}",
                    if pod_claim.containers.len() == 1 {
                        ""
                    } else {
                        "s"
                    },
                    pod_claim.containers.join(", ")
                ));
            }
            if let Some(parsed) = &parsed {
                let devices: Vec<&str> = parsed
                    .allocated()
                    .iter()
                    .map(|d| d.device.as_str())
                    .collect();
                if !devices.is_empty() {
                    facts.push(format!("devices {}", devices.join(", ")));
                }
                let classes = parsed.device_classes();
                if !classes.is_empty() {
                    facts.push(classes.join(", "));
                }
            }
            let mut card = card(colors).child(head);
            if !facts.is_empty() {
                card = card.child(
                    text_at(ix, facts.join(" · "), colors.text_dim)
                        .text_size(u(11.5))
                        .wrapped(),
                );
            }
            for (hx, (health, message)) in pod_claim.health.iter().enumerate() {
                let tone = match health.as_str() {
                    "Healthy" => Tone::Good,
                    "Unhealthy" => Tone::Bad,
                    _ => Tone::Muted,
                };
                let line = if message.is_empty() {
                    health.clone()
                } else {
                    format!("{health} · {message}")
                };
                card = card.child(
                    text_at(ix * 100 + hx, line, tone_color(tone, colors)).text_size(u(11.5)),
                );
            }
            body = body.child(card);
        }
        Some(
            section(format!("Resource claims · {}", claims.len()), colors)
                .child(body)
                .into_any_element(),
        )
    }

    /// A node's "Devices": its ResourceSlices and which claim holds each device.
    fn render_node_devices(
        &self,
        target: &Target,
        colors: &Colors,
        cx: &gpui::App,
    ) -> Option<AnyElement> {
        if !self.has_named("slices") {
            return None;
        }
        let slices = self.named("slices", cx);
        if slices.is_empty() {
            return None;
        }
        // Until the claims loaded, devices show no holder (not "free").
        let holders = self.holders(cx);
        let mut total = 0;
        let mut allocated = 0;
        let mut body = list().gap(u(3.0));
        let mut shown = 0;
        for (sx, slice_object) in slices.iter().enumerate() {
            let slice = Slice::parse(slice_object);
            let held = holders
                .as_deref()
                .map(|h| dra::allocations(&slice, h))
                .unwrap_or_default();
            total += slice.devices.len();
            allocated += held.len();
            let reference = self.reference(
                target,
                dra::GROUP,
                "resourceslices",
                None,
                name(slice_object),
                cx,
            );
            body = body.child(
                h_flex()
                    .gap(u(6.0))
                    .mt(u(if sx == 0 { 0.0 } else { 4.0 }))
                    .child(mono_link(
                        format!("node-slice-{sx}"),
                        name(slice_object).to_string(),
                        reference,
                        colors,
                    ))
                    .child(text_at(
                        sx,
                        format!(
                            "{} · pool {} · {} devices",
                            slice.driver,
                            slice.pool,
                            slice.devices.len()
                        ),
                        colors.text_dim,
                    )),
            );
            for device in &slice.devices {
                if shown >= DEVICE_LIMIT {
                    break;
                }
                let ix = shown;
                shown += 1;
                let holder = held
                    .iter()
                    .find(|(d, _)| d == &device.name)
                    .map(|(_, claims)| claims.as_slice());
                let mut row = h_flex()
                    .pl(u(12.0))
                    .gap(u(7.0))
                    .child(
                        Icon::new(IconName::Cpu)
                            .size(12.0)
                            .color(if holder.is_some() {
                                colors.green
                            } else {
                                colors.text_dim
                            }),
                    )
                    .child(mono_at(ix, device.name.clone(), colors.text))
                    .child(div().flex_1());
                row = match holder {
                    Some(claims) => row.child(self.holder_links(
                        &format!("node-device-{ix}"),
                        claims,
                        target,
                        colors,
                        cx,
                    )),
                    None if holders.is_some() => {
                        row.child(text_at(ix, "free", colors.text_dim).flex_none())
                    }
                    None => row,
                };
                body = body.child(row);
            }
        }
        if shown < total {
            body = body.child(empty(format!("… {} more devices", total - shown), colors));
        }
        let title = if holders.is_some() {
            format!("Devices · {total} · {allocated} allocated")
        } else {
            format!("Devices · {total}")
        };
        Some(section(title, colors).child(body).into_any_element())
    }
}

/// The distinct values, in order of first appearance.
fn distinct<'a>(values: impl IntoIterator<Item = &'a str>) -> Vec<&'a str> {
    let mut out: Vec<&str> = Vec::new();
    for value in values {
        if !out.contains(&value) {
            out.push(value);
        }
    }
    out
}
