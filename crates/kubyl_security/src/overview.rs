//! The Security card of the cluster and namespace overviews (board 23): findings by severity
//! across Images, Resources and Roles, from what the sidebar badge's read already holds.

use gpui::{
    AnyView, App, AppContext as _, Context, FontWeight, IntoElement, Render, Subscription, Window,
    div, prelude::*,
};
use kubyl_core::{ClusterId, OverviewSection};
use kubyl_kube::ConnectionManager;
use kubyl_security_core::kinds::View;
use kubyl_security_core::model::{Counts, Severity};
use kubyl_ui::{ActiveColors, Icon, IconName, StatusDot, h_flex, u, v_flex};

use crate::service::SecurityService;

/// Registered with `ChromeRegistry::add_overview_section`.
pub struct SecurityOverview;

impl OverviewSection for SecurityOverview {
    fn id(&self) -> &'static str {
        "security"
    }

    fn order(&self) -> i32 {
        11
    }

    fn build(&self, cluster: &ClusterId, namespace: Option<&str>, cx: &mut App) -> Option<AnyView> {
        // Only where Trivy Operator's reports are served.
        let serves =
            ConnectionManager::try_global(cx).is_some_and(|m| m.read(cx).caps(cluster).trivy.any());
        if !serves || SecurityService::global(cx).is_none() {
            return None;
        }
        let (cluster, namespace) = (cluster.clone(), namespace.map(str::to_string));
        Some(
            cx.new(|cx| SecurityCard::new(cluster, namespace, cx))
                .into(),
        )
    }
}

struct SecurityCard {
    cluster: ClusterId,
    namespace: Option<String>,
    _subscriptions: Vec<Subscription>,
}

impl SecurityCard {
    fn new(cluster: ClusterId, namespace: Option<String>, cx: &mut Context<Self>) -> Self {
        let subscriptions = SecurityService::global(cx)
            .map(|s| vec![cx.observe(&s, |_, _, cx| cx.notify())])
            .unwrap_or_default();
        Self {
            cluster,
            namespace,
            _subscriptions: subscriptions,
        }
    }
}

impl Render for SecurityCard {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let colors = cx.colors().clone();
        let summary = SecurityService::global(cx)
            .and_then(|s| s.read(cx).summary(&self.cluster, self.namespace.as_deref()));
        let Some(summary) = summary else {
            return div();
        };
        let total = summary.total();
        let open = self.cluster.clone();
        let pill = |severity: Severity, color| {
            h_flex()
                .gap(u(6.0))
                .text_size(u(12.5))
                .child(StatusDot::new(color))
                .child(
                    div()
                        .font_weight(FontWeight::SEMIBOLD)
                        .child(total.get(severity).to_string()),
                )
                .child(
                    div()
                        .text_color(colors.text_muted)
                        .child(severity.label().to_lowercase()),
                )
        };
        // Where the findings are: one line per view with its own worst counts.
        let line = |view: View, counts: Counts| {
            let cluster = self.cluster.clone();
            let selector = format!("overview-security-{}", view.id());
            h_flex()
                .id(gpui::SharedString::from(selector.clone()))
                .debug_selector(move || selector.clone())
                .gap(u(8.0))
                .h(u(22.0))
                .text_size(u(12.0))
                .cursor_pointer()
                .on_click(move |_, window, cx| crate::open(&cluster, view, window, cx))
                .child(
                    div()
                        .w(u(80.0))
                        .text_color(colors.text_muted)
                        .child(view.label()),
                )
                .children(
                    [
                        (Severity::Critical, colors.red),
                        (Severity::High, colors.orange),
                        (Severity::Medium, colors.yellow),
                        (Severity::Low, colors.accent),
                    ]
                    .map(|(severity, color)| {
                        let n = counts.get(severity);
                        div()
                            .w(u(52.0))
                            .font_family(kubyl_ui::fonts::MONO)
                            .text_color(if n == 0 { colors.text_faint } else { color })
                            .child(format!("{n} {}", &severity.label()[..1]))
                    }),
                )
        };
        v_flex()
            .debug_selector(|| "overview-security".into())
            .p(u(14.0))
            .gap(u(10.0))
            .rounded(u(8.0))
            .border_1()
            .border_color(colors.border)
            .bg(colors.panel)
            .child(
                h_flex()
                    .gap(u(8.0))
                    .child(Icon::new(IconName::Shield).size(14.0).color(colors.accent))
                    .child(
                        div()
                            .font_weight(FontWeight::SEMIBOLD)
                            .child(match &self.namespace {
                                Some(ns) => format!("Security in {ns}"),
                                None => "Security".to_string(),
                            }),
                    )
                    .child(div().flex_1())
                    .child(
                        div()
                            .id("overview-security-open")
                            .debug_selector(|| "overview-security-open".into())
                            .cursor_pointer()
                            .text_size(u(12.0))
                            .text_color(colors.accent)
                            .child("Open Security")
                            .on_click(move |_, window, cx| {
                                crate::open(&open, View::Images, window, cx)
                            }),
                    ),
            )
            .child(
                h_flex()
                    .gap(u(18.0))
                    .child(pill(Severity::Critical, colors.red))
                    .child(pill(Severity::High, colors.orange))
                    .child(pill(Severity::Medium, colors.yellow))
                    .child(pill(Severity::Low, colors.accent)),
            )
            .child(
                v_flex()
                    .child(line(View::Images, summary.images))
                    .child(line(View::Resources, summary.resources))
                    .child(line(View::Roles, summary.roles)),
            )
    }
}

#[cfg(test)]
mod tests {
    use gpui::TestAppContext;
    use kubyl_security_core::kinds::ReportKind;
    use kubyl_security_core::model::{Report, Subject};

    use super::*;

    fn report(kind: ReportKind, image: Option<&str>, critical: u32, high: u32) -> Report {
        Report {
            kind,
            namespace: Some("shop".into()),
            name: format!("r-{critical}"),
            subject: Subject {
                kind: "Pod".into(),
                name: "p".into(),
                namespace: Some("shop".into()),
            },
            container: None,
            image: image.map(str::to_string),
            scanner: "Trivy".into(),
            created: None,
            counts: Counts {
                critical,
                high,
                ..Default::default()
            },
        }
    }

    #[gpui::test]
    fn the_card_shows_severity_totals_once_reports_were_read(cx: &mut TestAppContext) {
        let dir = tempfile::tempdir().unwrap();
        let cluster = ClusterId::new("kind-sec@/k");
        let service = cx.update(|cx| {
            kubyl_core::init(cx);
            kubyl_settings::init_with_dir(cx, dir.path());
            kubyl_ui::init(cx);
            SecurityService::install(cx)
        });
        let (card, cx) = {
            let cluster = cluster.clone();
            cx.add_window_view(move |_, cx| SecurityCard::new(cluster, None, cx))
        };
        cx.run_until_parked();
        // Nothing read yet: no card (an empty one would suggest a clean cluster).
        assert!(cx.debug_bounds("overview-security").is_none());
        service.update(cx, |service, cx| {
            service.set_reports(
                &cluster,
                vec![
                    report(ReportKind::Vulnerability, Some("nginx:1.19"), 42, 143),
                    report(ReportKind::ConfigAudit, None, 0, 4),
                ],
                cx,
            )
        });
        cx.run_until_parked();
        assert!(cx.debug_bounds("overview-security").is_some());
        assert!(cx.debug_bounds("overview-security-open").is_some());
        assert!(cx.debug_bounds("overview-security-images").is_some());
        service.read_with(cx, |service, _| {
            let summary = service.summary(&cluster, None).unwrap();
            assert_eq!(summary.total().critical, 42);
            assert_eq!(summary.total().high, 147);
            assert_eq!(service.criticals(&cluster), Some(42));
            assert_eq!(
                service
                    .summary(&cluster, Some("elsewhere"))
                    .unwrap()
                    .total(),
                Counts::default()
            );
        });
        let _ = card;
    }
}
