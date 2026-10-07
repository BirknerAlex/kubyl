//! VerticalPodAutoscalers in the details: the target workload (a link), the update mode and,
//! per container, the recommendation (lower bound, target, upper bound) next to the requests
//! of the target's pod template.

use gpui::{AnyElement, Context, IntoElement, div, prelude::*};
use kubyl_core::Gvk;
use kubyl_kube::ConnectionManager;
use kubyl_resources::format::parse_quantity;
use kubyl_resources::vpa::{self, Resources, Vpa};
use kubyl_ui::{Colors, fonts, h_flex, u, v_flex};
use serde_json::Value;

use super::parts::{Wrapped, card, empty, kv_row, list, mono_link, text, text_at};
use super::{DetailsContent, Target, section};

impl DetailsContent {
    /// The target's served resource, from discovery (any kind with a pod template works).
    fn vpa_target_gvr(
        &self,
        target: &Target,
        vpa: &Vpa,
        cx: &gpui::App,
    ) -> Option<kubyl_core::Gvr> {
        let workload = vpa.target.as_ref()?;
        let (group, version) = workload.group_version();
        let discovery = ConnectionManager::try_global(cx)?
            .read(cx)
            .discovery(&target.cluster)?;
        discovery
            .by_gvk(&Gvk::new(group, version, workload.kind.clone()))
            .map(|info| info.gvr.clone())
            .or_else(|| {
                let resource = workload.resource()?;
                crate::catalog::find(&discovery, group, resource).map(|info| info.gvr.clone())
            })
    }

    pub(super) fn load_vpa(&mut self, target: &Target, object: &Value, cx: &mut Context<Self>) {
        let parsed = Vpa::parse(object);
        let (Some(workload), Some(gvr)) = (
            parsed.target.clone(),
            self.vpa_target_gvr(target, &parsed, cx),
        ) else {
            return;
        };
        let key =
            kubyl_resources::StoreKey::new(target.cluster.clone(), gvr, target.namespace.clone())
                .fields(format!("metadata.name={}", workload.name));
        let handle = self.acquire(key, cx);
        self.related.named.insert("target".into(), handle);
    }

    pub(super) fn render_vpa(
        &mut self,
        object: &Value,
        target: &Target,
        colors: &Colors,
        cx: &mut Context<Self>,
    ) -> Vec<AnyElement> {
        let parsed = Vpa::parse(object);
        let workload = self.named("target", cx).into_iter().next();
        let mut facts = v_flex().gap(u(4.0));
        match (&parsed.target, self.vpa_target_gvr(target, &parsed, cx)) {
            (Some(t), Some(gvr)) => {
                let reference = kubyl_core::ResourceRef::object(
                    target.cluster.clone(),
                    gvr,
                    target.namespace.clone(),
                    t.name.clone(),
                );
                facts = facts.child(kv_row(
                    "Workload",
                    86.0,
                    h_flex()
                        .gap(u(6.0))
                        .child(text(t.kind.clone(), colors.text_dim).flex_none())
                        .child(mono_link("vpa-target", t.name.clone(), reference, colors))
                        .when(self.has_named("target") && workload.is_none(), |this| {
                            this.child(text("not found", colors.yellow))
                        }),
                    colors,
                ));
            }
            (Some(t), None) => {
                facts = facts.child(kv_row(
                    "Workload",
                    86.0,
                    text(format!("{} {}", t.kind, t.name), colors.text),
                    colors,
                ));
            }
            (None, _) => {
                facts = facts.child(kv_row(
                    "Workload",
                    86.0,
                    text("none", colors.text_dim),
                    colors,
                ))
            }
        }
        let mode_note = match parsed.update_mode.as_str() {
            "Off" => " · recommendations only",
            "Initial" => " · applied when pods are created",
            "Recreate" | "Auto" => " · pods are evicted to apply",
            "InPlaceOrRecreate" => " · resized in place where possible",
            _ => "",
        };
        facts = facts.child(kv_row(
            "Update mode",
            86.0,
            text(format!("{}{mode_note}", parsed.update_mode), colors.text),
            colors,
        ));
        let mut out = vec![section("Target", colors).child(facts).into_any_element()];

        // One card per container of the template (or of the recommendation, without one).
        let requests = workload
            .as_deref()
            .map(vpa::template_requests)
            .unwrap_or_default();
        let mut containers: Vec<String> = requests.iter().map(|(c, _)| c.clone()).collect();
        for r in &parsed.recommendations {
            if !containers.contains(&r.container) {
                containers.push(r.container.clone());
            }
        }
        let mut body = list();
        if containers.is_empty() {
            body = body.child(
                empty(
                    "No recommendation yet: the recommender needs a few minutes of metrics.",
                    colors,
                )
                .wrapped(),
            );
        }
        for (ix, container) in containers.iter().enumerate() {
            let recommendation = parsed
                .recommendations
                .iter()
                .find(|r| &r.container == container);
            let current = requests
                .iter()
                .find(|(c, _)| c == container)
                .map(|(_, r)| r.clone());
            let mode = parsed.container_mode(container);
            let mut head = h_flex()
                .gap(u(6.0))
                .child(
                    text_at(ix, container.clone(), colors.text)
                        .font_weight(gpui::FontWeight::MEDIUM),
                )
                .child(div().flex_1());
            let note = match (mode, recommendation, &current) {
                (Some("Off"), _, _) => "mode Off · not scaled".to_string(),
                (_, None, _) => "no recommendation".to_string(),
                (_, Some(r), Some(current)) => compare(current, r.target.as_ref()),
                _ => String::new(),
            };
            head = head.child(text_at(ix, note, colors.text_dim).text_size(u(11.5)));
            let mut card = card(colors).child(head);
            if let Some(r) = recommendation {
                let header = |label: &'static str| {
                    div()
                        .flex_1()
                        .text_size(u(10.5))
                        .text_color(colors.text_dim)
                        .child(label.to_uppercase())
                };
                let mut grid = v_flex().gap(u(3.0)).child(
                    h_flex()
                        .gap(u(8.0))
                        .child(div().w(u(60.0)))
                        .child(header("Requests"))
                        .child(header("Lower"))
                        .child(header("Target"))
                        .child(header("Upper")),
                );
                for (rx, (label, pick)) in [("CPU", 0usize), ("Memory", 1usize)]
                    .into_iter()
                    .enumerate()
                {
                    let value = |res: Option<&Resources>| -> String {
                        res.map(|r| {
                            if pick == 0 {
                                r.cpu.clone()
                            } else {
                                r.memory.clone()
                            }
                        })
                        .filter(|v| !v.is_empty())
                        .unwrap_or_else(|| "—".into())
                    };
                    let cell = |ix2: usize, v: String, color| {
                        div()
                            .flex_1()
                            .font_family(fonts::MONO)
                            .text_size(u(11.5))
                            .text_color(color)
                            .child(kubyl_ui::Selectable::new(
                                gpui::ElementId::Name(format!("vpa-{ix}-{rx}-{ix2}").into()),
                                v,
                            ))
                    };
                    grid = grid.child(
                        h_flex()
                            .gap(u(8.0))
                            .text_size(u(12.0))
                            .child(div().w(u(60.0)).text_color(colors.text_dim).child(label))
                            .child(cell(0, value(current.as_ref()), colors.text_muted))
                            .child(cell(1, value(r.lower.as_ref()), colors.text_muted))
                            .child(cell(2, value(r.target.as_ref()), colors.accent))
                            .child(cell(3, value(r.upper.as_ref()), colors.text_muted)),
                    );
                }
                card = card.child(grid);
            }
            body = body.child(card);
        }
        let with = parsed.recommendations.len();
        out.push(
            section(
                format!(
                    "Recommendations · {with} of {} containers",
                    containers.len().max(with)
                ),
                colors,
            )
            .child(body)
            .into_any_element(),
        );
        out
    }
}

/// `target above requests`, `target below requests`, `matches requests`.
fn compare(current: &Resources, target: Option<&Resources>) -> String {
    let Some(target) = target else {
        return String::new();
    };
    let cmp = |a: &str, b: &str| -> Option<std::cmp::Ordering> {
        parse_quantity(a)?.partial_cmp(&parse_quantity(b)?)
    };
    let cpu = cmp(&target.cpu, &current.cpu);
    let memory = cmp(&target.memory, &current.memory);
    use std::cmp::Ordering::*;
    match (cpu, memory) {
        (Some(Greater), _) | (_, Some(Greater)) => "target above requests".into(),
        (Some(Less), Some(Less)) => "target below requests".into(),
        (Some(Equal), Some(Equal)) => "matches requests".into(),
        (None, None) if current.cpu.is_empty() && current.memory.is_empty() => {
            "no requests set".into()
        }
        _ => String::new(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn recommendations_compare_with_requests() {
        let r = |cpu: &str, memory: &str| Resources {
            cpu: cpu.into(),
            memory: memory.into(),
        };
        assert_eq!(
            compare(&r("50m", "64Mi"), Some(&r("80m", "96Mi"))),
            "target above requests"
        );
        assert_eq!(
            compare(&r("1", "1Gi"), Some(&r("500m", "512Mi"))),
            "target below requests"
        );
        assert_eq!(
            compare(&r("", ""), Some(&r("500m", "512Mi"))),
            "no requests set"
        );
    }
}
