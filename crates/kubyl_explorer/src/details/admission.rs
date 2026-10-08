//! Admission control in the details: a policy's match constraints, match conditions,
//! variables, validations or mutations and audit annotations (CEL in monospace, no
//! highlighting yet), its bindings with their param objects; a binding's policy and what it
//! matches (the namespaces its selector picks); the webhooks of a webhook configuration with
//! their match conditions.

use std::sync::Arc;

use gpui::{AnyElement, App, Context, IntoElement, SharedString, div, prelude::*};
use kubyl_core::{Gvk, Gvr, ResourceRef};
use kubyl_kube::ConnectionManager;
use kubyl_resources::admission::{self, Binding, Match, Named, Policy};
use kubyl_resources::format::name;
use kubyl_ui::{Chip, Colors, Icon, IconName, h_flex, u, v_flex};
use serde_json::Value;

use super::parts::{Wrapped, card, cel, empty, kv_row, list, mono_at, mono_link, text, text_at};
use super::{DetailsContent, Target, section};

/// Namespaces listed for a binding's selector before "… N more".
const NAMESPACE_LIMIT: usize = 12;

impl DetailsContent {
    pub(super) fn load_admission(
        &mut self,
        target: &Target,
        object: &Value,
        cx: &mut Context<Self>,
    ) {
        let kind = target.kind.as_str();
        if admission::is_policy(kind) {
            let resource = admission::bindings_resource(kind);
            self.watch_named(
                "bindings",
                target,
                (admission::GROUP, resource),
                None,
                None,
                cx,
            );
        } else if admission::is_binding(kind) {
            let binding = Binding::parse(object);
            if !binding.policy.is_empty() {
                let fields = format!("metadata.name={}", binding.policy);
                self.watch_named(
                    "policy",
                    target,
                    (admission::GROUP, admission::policy_resource(kind)),
                    None,
                    Some(fields),
                    cx,
                );
            }
            if binding
                .resources
                .as_ref()
                .is_some_and(|m| m.namespace_selector.is_some())
            {
                self.watch_named("namespaces", target, ("", "namespaces"), None, None, cx);
            }
        }
    }

    pub(super) fn render_admission(
        &mut self,
        object: &Value,
        target: &Target,
        colors: &Colors,
        cx: &mut Context<Self>,
    ) -> Vec<AnyElement> {
        let kind = target.kind.as_str();
        if admission::is_policy(kind) {
            self.render_policy(object, target, colors, cx)
        } else if admission::is_binding(kind) {
            self.render_binding(object, target, colors, cx)
        } else if matches!(
            kind,
            "ValidatingWebhookConfiguration" | "MutatingWebhookConfiguration"
        ) {
            vec![render_webhooks(object, colors)]
        } else {
            Vec::new()
        }
    }

    fn render_policy(
        &self,
        object: &Value,
        target: &Target,
        colors: &Colors,
        cx: &App,
    ) -> Vec<AnyElement> {
        let policy = Policy::parse(object);
        let mut out = Vec::new();
        if let Some(constraints) = &policy.constraints {
            out.push(match_section(
                "Match constraints",
                constraints,
                None,
                colors,
            ));
        }
        if let Some(section) = named_section(
            "Match conditions",
            "condition",
            &policy.match_conditions,
            true,
            colors,
        ) {
            out.push(section);
        }
        if let Some(section) =
            named_section("Variables", "variable", &policy.variables, false, colors)
        {
            out.push(section);
        }
        if target.kind == "ValidatingAdmissionPolicy" {
            let mut body = list();
            if policy.validations.is_empty() {
                body = body.child(empty("No validations.", colors));
            }
            for (ix, validation) in policy.validations.iter().enumerate() {
                let mut card = card(colors).child(cel(
                    SharedString::from(format!("validation-{ix}")),
                    &validation.expression,
                    colors,
                ));
                let mut facts = Vec::new();
                if !validation.message.is_empty() {
                    facts.push(validation.message.clone());
                }
                if !validation.message_expression.is_empty() {
                    facts.push(format!("message: {}", validation.message_expression));
                }
                if !validation.reason.is_empty() {
                    facts.push(format!("reason {}", validation.reason));
                }
                if !facts.is_empty() {
                    card = card.child(
                        text_at(ix, facts.join(" · "), colors.text_dim)
                            .text_size(u(11.5))
                            .wrapped(),
                    );
                }
                body = body.child(card);
            }
            out.push(
                section(
                    format!("Validations · {}", policy.validations.len()),
                    colors,
                )
                .child(body)
                .into_any_element(),
            );
        } else {
            let mut body = list();
            if policy.mutations.is_empty() {
                body = body.child(empty("No mutations.", colors));
            }
            for (ix, mutation) in policy.mutations.iter().enumerate() {
                body = body.child(
                    card(colors)
                        .child(
                            text_at(ix, mutation.patch_type.clone(), colors.text_dim)
                                .text_size(u(11.5)),
                        )
                        .child(cel(
                            SharedString::from(format!("mutation-{ix}")),
                            &mutation.expression,
                            colors,
                        )),
                );
            }
            out.push(
                section(format!("Mutations · {}", policy.mutations.len()), colors)
                    .child(body)
                    .into_any_element(),
            );
        }
        if !policy.audit_annotations.is_empty() {
            let mut body = v_flex().gap(u(4.0));
            for (ix, (key, value)) in policy.audit_annotations.iter().enumerate() {
                body = body.child(kv_row(
                    key.clone(),
                    90.0,
                    cel(SharedString::from(format!("audit-{ix}")), value, colors),
                    colors,
                ));
            }
            out.push(
                section(
                    format!("Audit annotations · {}", policy.audit_annotations.len()),
                    colors,
                )
                .child(body)
                .into_any_element(),
            );
        }
        if !policy.warnings.is_empty() {
            let mut body = list().gap(u(4.0));
            for (ix, (field, warning)) in policy.warnings.iter().enumerate() {
                body = body.child(
                    h_flex()
                        .items_start()
                        .gap(u(6.0))
                        .child(
                            div().pt(u(2.0)).child(
                                Icon::new(IconName::TriangleAlert)
                                    .size(12.0)
                                    .color(colors.yellow),
                            ),
                        )
                        .child(
                            v_flex()
                                .min_w_0()
                                .child(mono_at(ix, field.clone(), colors.text_dim))
                                .child(text_at(ix, warning.clone(), colors.yellow).wrapped()),
                        ),
                );
            }
            out.push(
                section("Type checking", colors)
                    .child(body)
                    .into_any_element(),
            );
        }

        // Bindings.
        if self.has_named("bindings") {
            let policy_name = name(object).to_string();
            let mine = self.derived("bindings", &["bindings"], cx, |this| {
                // `named` sorts by name, like `admission::bindings_of`.
                this.named("bindings", cx)
                    .into_iter()
                    .filter(|b| {
                        kubyl_resources::format::str_at(b, "/spec/policyName") == policy_name
                    })
                    .collect::<Vec<Arc<Value>>>()
            });
            let mut body = list();
            if mine.is_empty() {
                body = body.child(
                    text(
                        "No binding: the policy doesn't apply to anything yet.",
                        colors.yellow,
                    )
                    .text_size(u(12.0))
                    .wrapped(),
                );
            }
            for (ix, binding_object) in mine.iter().enumerate() {
                body =
                    body.child(self.binding_card(ix, binding_object, &policy, target, colors, cx));
            }
            out.push(
                section(format!("Bindings · {}", mine.len()), colors)
                    .child(body)
                    .into_any_element(),
            );
        }
        out
    }

    fn binding_card(
        &self,
        ix: usize,
        binding_object: &Value,
        policy: &Policy,
        target: &Target,
        colors: &Colors,
        cx: &App,
    ) -> impl IntoElement {
        let binding = Binding::parse(binding_object);
        let reference = self.reference(
            target,
            admission::GROUP,
            admission::bindings_resource(&target.kind),
            None,
            name(binding_object),
            cx,
        );
        let mut head = h_flex()
            .gap(u(6.0))
            .child(Icon::new(IconName::Link).size(12.0).color(colors.text_dim))
            .child(mono_link(
                format!("binding-{ix}"),
                name(binding_object).to_string(),
                reference,
                colors,
            ))
            .child(div().flex_1());
        for (ax, action) in binding.validation_actions.iter().enumerate() {
            let chip = Chip::new(action.clone())
                .selectable_as(SharedString::from(format!("binding-{ix}-action-{ax}")));
            head = head.child(if action == "Deny" {
                chip.dot(colors.red)
            } else {
                chip
            });
        }
        let mut card = card(colors).child(head);
        if let Some(param) = &binding.param_ref {
            let mut row = h_flex().gap(u(5.0)).text_size(u(11.5)).child(text_at(
                ix,
                "params",
                colors.text_dim,
            ));
            row = match self.param_ref(policy, param, target, cx) {
                (label, Some(reference)) => row.child(mono_link(
                    format!("binding-{ix}-param"),
                    label,
                    reference,
                    colors,
                )),
                (label, None) => row.child(mono_at(ix, label, colors.text)),
            };
            if !param.not_found.is_empty() {
                row = row.child(text_at(
                    ix,
                    format!("· missing params: {}", param.not_found),
                    colors.text_dim,
                ));
            }
            card = card.child(row);
        }
        if let Some(resources) = &binding.resources {
            let mut parts = Vec::new();
            if let Some(selector) = &resources.namespace_selector {
                parts.push(format!("namespaces {}", admission::selector_text(selector)));
            }
            if let Some(selector) = &resources.object_selector {
                parts.push(format!("objects {}", admission::selector_text(selector)));
            }
            if !resources.rules.is_empty() {
                parts.push(resources.rules.join("; "));
            }
            if !parts.is_empty() {
                card = card.child(
                    text_at(ix, parts.join(" · "), colors.text_dim)
                        .text_size(u(11.5))
                        .wrapped(),
                );
            }
        }
        card
    }

    /// A param ref's label and, when it names one object of a served kind, a link to it. A
    /// namespaced param kind without a namespace takes the params from each request's
    /// namespace: no link then.
    fn param_ref(
        &self,
        policy: &Policy,
        param: &admission::ParamRef,
        target: &Target,
        cx: &App,
    ) -> (String, Option<ResourceRef>) {
        let info = policy.param_kind.as_ref().and_then(|(api_version, kind)| {
            let (group, version) = match api_version.split_once('/') {
                Some((g, v)) => (g.to_string(), v.to_string()),
                None => (String::new(), api_version.clone()),
            };
            ConnectionManager::try_global(cx)?
                .read(cx)
                .discovery(&target.cluster)?
                .by_gvk(&Gvk::new(group, version, kind.clone()))
                .cloned()
        });
        let Some(info) = info else {
            return (param.label(), None);
        };
        let label = param.label_for(info.namespaced);
        if !param.names_one(info.namespaced) {
            return (label, None);
        }
        let namespace = info.namespaced.then(|| param.namespace.clone());
        let reference = ResourceRef::object(
            target.cluster.clone(),
            info.gvr.clone(),
            namespace,
            param.name.clone(),
        );
        (label, Some(reference))
    }

    fn render_binding(
        &self,
        object: &Value,
        target: &Target,
        colors: &Colors,
        cx: &App,
    ) -> Vec<AnyElement> {
        let binding = Binding::parse(object);
        let policy_object = self.named("policy", cx).into_iter().next();
        let policy = policy_object
            .as_deref()
            .map(Policy::parse)
            .unwrap_or_default();
        let mut facts = v_flex().gap(u(4.0));
        let reference = self.reference(
            target,
            admission::GROUP,
            admission::policy_resource(&target.kind),
            None,
            binding.policy.clone(),
            cx,
        );
        let mut policy_row = h_flex().gap(u(6.0)).child(mono_link(
            "binding-policy",
            binding.policy.clone(),
            reference,
            colors,
        ));
        if self.has_named("policy") && policy_object.is_none() {
            policy_row = policy_row.child(text("not found", colors.yellow));
        }
        facts = facts.child(kv_row("Policy", 96.0, policy_row, colors));
        if !binding.validation_actions.is_empty() {
            facts = facts.child(kv_row(
                "Actions",
                96.0,
                text(binding.validation_actions.join(", "), colors.text),
                colors,
            ));
        }
        if let Some(param) = &binding.param_ref {
            let value = match self.param_ref(&policy, param, target, cx) {
                (label, Some(reference)) => {
                    mono_link("binding-param", label, reference, colors).into_any_element()
                }
                (label, None) => text(label, colors.text).into_any_element(),
            };
            facts = facts.child(kv_row("Param ref", 96.0, value, colors));
            if let Some(kind) = policy.param_label() {
                facts = facts.child(kv_row("Param kind", 96.0, text(kind, colors.text), colors));
            }
            if !param.not_found.is_empty() {
                facts = facts.child(kv_row(
                    "Missing params",
                    96.0,
                    text(param.not_found.clone(), colors.text),
                    colors,
                ));
            }
        }
        let mut out = vec![section("Binding", colors).child(facts).into_any_element()];

        let namespaces = self.has_named("namespaces").then(|| {
            self.derived("namespaces", &["namespaces"], cx, |this| {
                this.named("namespaces", cx)
            })
        });
        match &binding.resources {
            Some(resources) => out.push(match_section(
                "Matches",
                resources,
                namespaces.as_deref().map(|n| (n.as_slice(), target)),
                colors,
            )),
            None => out.push(
                section("Matches", colors)
                    .child(empty(
                        "Everything the policy's match constraints select.",
                        colors,
                    ))
                    .into_any_element(),
            ),
        }
        out
    }
}

/// Rules, selectors and match policy of a policy's constraints or a binding's resources; with
/// the cluster's namespaces, the ones a namespace selector picks (links).
fn match_section(
    title: &str,
    m: &Match,
    namespaces: Option<(&[Arc<Value>], &Target)>,
    colors: &Colors,
) -> AnyElement {
    let mut body = v_flex().gap(u(4.0));
    // Both lists share this call site: offset the second one's indices so element ids differ.
    let rules = |rules: &[String], offset: usize| {
        let mut col = v_flex().gap(u(2.0));
        for (ix, rule) in rules.iter().enumerate() {
            col = col.child(mono_at(offset + ix, rule.clone(), colors.text).wrapped());
        }
        col
    };
    if !m.rules.is_empty() {
        body = body.child(kv_row("Resources", 90.0, rules(&m.rules, 0), colors));
    }
    if !m.excluded.is_empty() {
        body = body.child(kv_row(
            "Excluded",
            90.0,
            rules(&m.excluded, m.rules.len()),
            colors,
        ));
    }
    if let Some(selector) = &m.namespace_selector {
        body = body.child(kv_row(
            "Namespaces",
            90.0,
            text(admission::selector_text(selector), colors.text),
            colors,
        ));
        if let Some((namespaces, target)) = namespaces {
            let matching: Vec<&Arc<Value>> = namespaces
                .iter()
                .filter(|ns| {
                    admission::selector_matches(
                        selector,
                        ns.pointer("/metadata/labels").unwrap_or(&Value::Null),
                    )
                })
                .collect();
            let mut row = h_flex().flex_wrap().gap(u(6.0)).text_size(u(12.0));
            if matching.is_empty() {
                row = row.child(text("no namespace matches", colors.yellow));
            }
            for (ix, ns) in matching.iter().take(NAMESPACE_LIMIT).enumerate() {
                let reference = ResourceRef::object(
                    target.cluster.clone(),
                    Gvr::new("", "v1", "namespaces"),
                    None,
                    name(ns).to_string(),
                );
                row = row.child(mono_link(
                    format!("match-ns-{ix}"),
                    name(ns).to_string(),
                    reference,
                    colors,
                ));
            }
            if matching.len() > NAMESPACE_LIMIT {
                row = row.child(text(
                    format!("… {} more", matching.len() - NAMESPACE_LIMIT),
                    colors.text_dim,
                ));
            }
            body = body.child(kv_row("Matching", 90.0, row, colors));
        }
    }
    if let Some(selector) = &m.object_selector {
        body = body.child(kv_row(
            "Objects",
            90.0,
            text(admission::selector_text(selector), colors.text),
            colors,
        ));
    }
    if !m.match_policy.is_empty() {
        body = body.child(kv_row(
            "Match policy",
            90.0,
            text(m.match_policy.clone(), colors.text),
            colors,
        ));
    }
    if m.rules.is_empty() && m.namespace_selector.is_none() && m.object_selector.is_none() {
        body = body.child(empty("Everything.", colors));
    }
    section(title, colors).child(body).into_any_element()
}

/// Named CEL expressions as cards (`cards`) or as rows.
fn named_section(
    title: &str,
    id: &str,
    items: &[Named],
    cards: bool,
    colors: &Colors,
) -> Option<AnyElement> {
    if items.is_empty() {
        return None;
    }
    let mut body = list();
    for (ix, item) in items.iter().enumerate() {
        let expression = cel(
            SharedString::from(format!("{id}-{ix}")),
            &item.expression,
            colors,
        );
        body = body.child(if cards {
            card(colors)
                .child(
                    text_at(ix, item.name.clone(), colors.text)
                        .font_weight(gpui::FontWeight::MEDIUM),
                )
                .child(expression)
                .into_any_element()
        } else {
            kv_row(item.name.clone(), 90.0, expression, colors).into_any_element()
        });
    }
    Some(
        section(format!("{title} · {}", items.len()), colors)
            .child(body)
            .into_any_element(),
    )
}

/// The webhooks of a configuration with their rules, selectors and match conditions.
fn render_webhooks(object: &Value, colors: &Colors) -> AnyElement {
    let hooks = admission::webhooks(object);
    let mut body = list();
    if hooks.is_empty() {
        body = body.child(empty("No webhooks.", colors));
    }
    for (ix, hook) in hooks.iter().enumerate() {
        let mut facts = Vec::new();
        if !hook.failure_policy.is_empty() {
            facts.push(format!("failure {}", hook.failure_policy));
        }
        if !hook.side_effects.is_empty() {
            facts.push(format!("side effects {}", hook.side_effects));
        }
        if hook.timeout > 0 {
            facts.push(format!("timeout {}s", hook.timeout));
        }
        if !hook.reinvocation_policy.is_empty() {
            facts.push(format!("reinvocation {}", hook.reinvocation_policy));
        }
        let mut card = card(colors)
            .child(
                text_at(ix, hook.name.clone(), colors.text).font_weight(gpui::FontWeight::MEDIUM),
            )
            .child(mono_at(ix, hook.client.clone(), colors.text_muted))
            .child(text_at(ix, facts.join(" · "), colors.text_dim).text_size(u(11.5)));
        for (rx, rule) in hook.rules.iter().enumerate() {
            card = card.child(mono_at(ix * 100 + rx, rule.clone(), colors.text));
        }
        if let Some(selector) = &hook.namespace_selector {
            card = card.child(
                text_at(
                    ix,
                    format!("namespaces {}", admission::selector_text(selector)),
                    colors.text_dim,
                )
                .text_size(u(11.5)),
            );
        }
        if let Some(selector) = &hook.object_selector {
            card = card.child(
                text_at(
                    ix,
                    format!("objects {}", admission::selector_text(selector)),
                    colors.text_dim,
                )
                .text_size(u(11.5)),
            );
        }
        if !hook.match_conditions.is_empty() {
            card = card.child(
                text_at(
                    ix,
                    format!("Match conditions · {}", hook.match_conditions.len()),
                    colors.text_dim,
                )
                .text_size(u(11.5))
                .mt(u(2.0)),
            );
            for (cx_ix, condition) in hook.match_conditions.iter().enumerate() {
                card = card
                    .child(mono_at(
                        ix * 100 + cx_ix,
                        condition.name.clone(),
                        colors.text,
                    ))
                    .child(cel(
                        SharedString::from(format!("webhook-{ix}-condition-{cx_ix}")),
                        &condition.expression,
                        colors,
                    ));
            }
        }
        body = body.child(card);
    }
    section(format!("Webhooks · {}", hooks.len()), colors)
        .child(body)
        .into_any_element()
}
