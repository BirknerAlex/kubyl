//! The alert details pane: times, summary and description (plain text), the target (checked
//! against the live object), labels, annotations, runbook, routing, the rule and the last 24 h.

use std::collections::BTreeMap;

use gpui::{
    AnyElement, Context, FontWeight, IntoElement, Modifiers, SharedString, Window, div, prelude::*,
    px,
};
use gpui_component::button::{Button as MenuButton, ButtonVariants as _};
use gpui_component::menu::{DropdownMenu as _, PopupMenuItem};
use jiff::Timestamp;
use kubyl_core::{ClusterId, spawn_kube};
use kubyl_metrics::prometheus::RangeSeries;
use kubyl_ui::{ActiveColors, Button, Colors, Icon, IconName, fonts, h_flex, u, v_flex};
use serde_json::Value;

use super::alerts_tab::target_icon;
use super::{AlertsView, Entry, Tab, widgets};
use crate::matchers;
use crate::model::{Alert, AlertState, Rule, Target};

/// What the pane loaded for the selected alert.
#[derive(Default)]
pub(crate) struct DetailsState {
    /// The alert these are for.
    for_alert: Option<String>,
    target: TargetCheck,
    timeline: Timeline,
    description_open: bool,
}

#[derive(Clone, Debug, Default, PartialEq)]
enum TargetCheck {
    #[default]
    Unknown,
    Checking,
    /// Found, with a short status (`CrashLoopBackOff`, `Running`).
    Found(Option<String>),
    NotFound,
    /// Couldn't tell (forbidden…): the link still shows.
    Unchecked,
}

#[derive(Clone, Debug, Default, PartialEq)]
enum Timeline {
    #[default]
    Unknown,
    Loading,
    /// 24 h in slots: firing, pending or nothing; and how often it started firing.
    Ready {
        slots: Vec<Option<AlertState>>,
        fired: usize,
    },
    Unavailable,
}

const TIMELINE_STEP: f64 = 300.0;
const TIMELINE_SPAN: f64 = 86_400.0;

/// The 24 h timeline of an alert from `ALERTS` series: a series belongs to it when its labels
/// (without `alertstate`) are all in the alert's labels.
fn timeline(
    series: &[RangeSeries],
    labels: &BTreeMap<String, String>,
    start: f64,
    end: f64,
    step: f64,
) -> (Vec<Option<AlertState>>, usize) {
    let slots = ((end - start) / step).round() as usize + 1;
    let mut out: Vec<Option<AlertState>> = vec![None; slots];
    for s in series {
        let own = s
            .labels
            .iter()
            .filter(|(k, _)| k.as_str() != "alertstate" && k.as_str() != "__name__")
            .all(|(k, v)| labels.get(k) == Some(v));
        if !own {
            continue;
        }
        let state = match s.labels.get("alertstate").map(String::as_str) {
            Some("firing") => AlertState::Firing,
            Some("pending") => AlertState::Pending,
            _ => continue,
        };
        for (t, _) in &s.values {
            let i = ((t - start) / step).round();
            if i < 0.0 {
                continue;
            }
            if let Some(slot) = out.get_mut(i as usize) {
                // Firing wins over pending.
                if *slot != Some(AlertState::Firing) {
                    *slot = Some(state);
                }
            }
        }
    }
    let mut fired = 0;
    let mut previous = None;
    for slot in &out {
        if *slot == Some(AlertState::Firing) && previous != Some(AlertState::Firing) {
            fired += 1;
        }
        previous = *slot;
    }
    (out, fired)
}

/// The rule behind an alert: same name, and its labels are all in the alert's.
pub(crate) fn rule_of(alert: &Alert, groups: &[crate::model::RuleGroup]) -> Option<Rule> {
    groups
        .iter()
        .flat_map(|g| &g.rules)
        .filter(|r| r.name == alert.name)
        .find(|r| r.labels.iter().all(|(k, v)| alert.labels.get(k) == Some(v)))
        .cloned()
}

/// The API path of a target object.
pub(crate) fn object_path(target: &Target) -> String {
    let (group, version, resource) = target.kind.gvr();
    let base = if group.is_empty() {
        format!("/api/{version}")
    } else {
        format!("/apis/{group}/{version}")
    };
    match (&target.namespace, target.kind.namespaced()) {
        (Some(ns), true) => format!("{base}/namespaces/{ns}/{resource}/{}", target.name),
        _ => format!("{base}/{resource}/{}", target.name),
    }
}

/// A short status of an object: a waiting container's reason, else the pod's phase; a node's
/// readiness.
fn object_status(object: &Value) -> Option<String> {
    if let Some(statuses) = object
        .pointer("/status/containerStatuses")
        .and_then(Value::as_array)
    {
        for status in statuses {
            if let Some(reason) = status
                .pointer("/state/waiting/reason")
                .and_then(Value::as_str)
            {
                return Some(reason.to_string());
            }
        }
    }
    if let Some(conditions) = object
        .pointer("/status/conditions")
        .and_then(Value::as_array)
        && object["kind"] == "Node"
    {
        let ready = conditions
            .iter()
            .find(|c| c["type"] == "Ready")
            .map(|c| c["status"] == "True");
        return ready.map(|r| if r { "Ready".into() } else { "NotReady".into() });
    }
    object
        .pointer("/status/phase")
        .and_then(Value::as_str)
        .map(str::to_string)
}

impl AlertsView {
    /// Starts the target check and the timeline for the selected alert (once).
    fn load_details(&mut self, entry: &Entry, cx: &mut Context<Self>) {
        if self.details_state.for_alert.as_ref() == Some(&entry.alert.fingerprint) {
            return;
        }
        self.details_state = DetailsState {
            for_alert: Some(entry.alert.fingerprint.clone()),
            ..Default::default()
        };
        let client = kubyl_kube::ConnectionManager::try_global(cx)
            .and_then(|m| m.read(cx).client(&entry.cluster));
        let Some(client) = client else {
            return;
        };
        if let Some(target) = entry.alert.target.clone() {
            self.details_state.target = TargetCheck::Checking;
            let path = object_path(&target);
            let client = client.clone();
            let task = spawn_kube(cx, async move {
                let request = http::Request::get(path).body(Vec::new()).ok()?;
                Some(client.request::<Value>(request).await)
            });
            let fingerprint = entry.alert.fingerprint.clone();
            cx.spawn(async move |this, cx| {
                let result = task.await;
                this.update(cx, |this, cx| {
                    if this.details_state.for_alert.as_ref() != Some(&fingerprint) {
                        return;
                    }
                    this.details_state.target = match result {
                        Some(Ok(object)) => TargetCheck::Found(object_status(&object)),
                        Some(Err(kube::Error::Api(status))) if status.code == 404 => {
                            TargetCheck::NotFound
                        }
                        _ => TargetCheck::Unchecked,
                    };
                    cx.notify();
                })
                .ok();
            })
            .detach();
        }
        let prom = self
            .state(&entry.cluster, cx)
            .and_then(|s| s.prometheus().cloned());
        let Some(prom) = prom else {
            self.details_state.timeline = Timeline::Unavailable;
            return;
        };
        self.details_state.timeline = Timeline::Loading;
        let name = entry.alert.name.replace('\\', "\\\\").replace('"', "\\\"");
        let labels = entry.alert.labels.clone();
        let fingerprint = entry.alert.fingerprint.clone();
        let task = spawn_kube(cx, async move {
            let end = (Timestamp::now().as_second() as f64 / TIMELINE_STEP).floor() * TIMELINE_STEP;
            let start = end - TIMELINE_SPAN;
            let series = prom
                .query_range(
                    &format!("ALERTS{{alertname=\"{name}\"}}"),
                    start,
                    end,
                    TIMELINE_STEP,
                )
                .await;
            series.map(|s| timeline(&s, &labels, start, end, TIMELINE_STEP))
        });
        cx.spawn(async move |this, cx| {
            let result = task.await;
            this.update(cx, |this, cx| {
                if this.details_state.for_alert.as_ref() != Some(&fingerprint) {
                    return;
                }
                this.details_state.timeline = match result {
                    Ok((slots, fired)) => Timeline::Ready { slots, fired },
                    Err(_) => Timeline::Unavailable,
                };
                cx.notify();
            })
            .ok();
        })
        .detach();
    }

    /// Adds `name="value"` (or `!=` with alt) to the filter.
    fn add_label_filter(
        &mut self,
        name: &str,
        value: &str,
        exclude: bool,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let op = if exclude {
            matchers::MatchOp::NotEqual
        } else {
            matchers::MatchOp::Equal
        };
        let matcher = matchers::Matcher::new(name, op, value).to_string();
        let current = self.filter_input.read(cx).value().trim().to_string();
        let text = if current.is_empty()
            || !matches!(self.filters.query, super::rows::Query::Matchers(_))
        {
            matcher
        } else {
            format!("{current}, {matcher}")
        };
        self.filter_input
            .update(cx, |input, cx| input.set_value(text, window, cx));
    }

    pub(crate) fn render_details(
        &mut self,
        entry: Entry,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let _ = window;
        self.load_details(&entry, cx);
        let colors = cx.colors().clone();
        let now = Timestamp::now();
        let alert = entry.alert.clone();
        let cluster = entry.cluster.clone();
        let state = self.state(&cluster, cx);
        let has_am = state.is_some_and(|s| s.has_alertmanager());
        let rule = state.and_then(|s| rule_of(&alert, &s.rules));
        let silences: Vec<crate::model::Silence> = state
            .map(|s| {
                s.silences
                    .iter()
                    .filter(|sil| alert.silenced_by.contains(&sil.id))
                    .cloned()
                    .collect()
            })
            .unwrap_or_default();
        let read_only = self.read_only(&cluster, cx);
        let can_silence = has_am && !read_only && alert.state != AlertState::Resolved;

        // Title block.
        let fired = match &self.details_state.timeline {
            Timeline::Ready { fired, .. } if *fired > 1 => {
                Some(format!("fired {fired} times in 24 h"))
            }
            _ => None,
        };
        let title = v_flex()
            .px(u(14.0))
            .py(u(12.0))
            .gap(u(8.0))
            .border_b_1()
            .border_color(colors.border_variant)
            .child(
                div()
                    .font_family(fonts::MONO)
                    .text_size(u(13.5))
                    .font_weight(FontWeight::SEMIBOLD)
                    .child(alert.name.clone()),
            )
            .child(
                h_flex()
                    .gap(u(10.0))
                    .text_size(u(12.0))
                    .child(widgets::severity_pill(&alert.severity, &colors))
                    .child(widgets::state_label(alert.state, &colors))
                    .when_some(fired, |this, fired| {
                        this.child(div().text_color(colors.text_dim).child(fired))
                    }),
            )
            .child(self.render_detail_actions(&entry, can_silence, cx));

        // Times.
        let mut times: Vec<(&'static str, AnyElement)> = Vec::new();
        let time = |t: Timestamp, approx: bool| {
            div()
                .child(format!(
                    "{}{} · {}",
                    if approx { "~" } else { "" },
                    widgets::ago(Some(t), now),
                    widgets::local_and_utc(t)
                ))
                .into_any_element()
        };
        if alert.state == AlertState::Resolved {
            if let Some(end) = alert.ends_at {
                times.push(("Resolved at", time(end, false)));
                if let Some(start) = alert.starts_at {
                    times.push((
                        "Duration",
                        div()
                            .child(widgets::short_duration(end.duration_since(start).as_secs()))
                            .into_any_element(),
                    ));
                }
            }
        } else if let Some(start) = alert
            .starts_at
            .filter(|_| alert.state != AlertState::Pending)
        {
            times.push(("Firing since", time(start, alert.starts_approx)));
        }
        if let Some(active) = alert.active_at {
            times.push(("Pending since", time(active, false)));
        }
        if let Some(updated) = alert.updated_at {
            times.push((
                "Last received",
                div()
                    .child(format!("{} ago", widgets::ago(Some(updated), now)))
                    .into_any_element(),
            ));
        }
        if let Some(value) = &alert.value {
            times.push((
                "Value",
                div()
                    .font_family(fonts::MONO)
                    .child(widgets::format_value(value))
                    .into_any_element(),
            ));
        }
        if !alert.receivers.is_empty() {
            times.push((
                "Receivers",
                h_flex()
                    .flex_wrap()
                    .gap(u(4.0))
                    .children(alert.receivers.iter().map(|r| {
                        div()
                            .px(u(6.0))
                            .rounded(u(4.0))
                            .bg(colors.chip_background)
                            .text_size(u(11.5))
                            .child(r.clone())
                    }))
                    .into_any_element(),
            ));
        }
        if self.cluster.is_none() {
            times.push((
                "Cluster",
                div()
                    .child(Self::cluster_name(&cluster, cx))
                    .into_any_element(),
            ));
        }
        times.push((
            "Source",
            div()
                .text_color(colors.text_muted)
                .child(alert.source.clone())
                .into_any_element(),
        ));
        let times = v_flex()
            .px(u(14.0))
            .py(u(12.0))
            .border_b_1()
            .border_color(colors.border_variant)
            .child(widgets::kv(times, &colors));

        // Summary and description.
        let summary = alert.summary().to_string();
        let description = alert.description().map(str::to_string);
        let summary_section = (!summary.is_empty() || description.is_some()).then(|| {
            let open = self.details_state.description_open;
            let long = description
                .as_ref()
                .is_some_and(|d| d.chars().count() > 240);
            let shown = match (&description, long && !open) {
                (Some(d), true) => Some(format!("{}…", d.chars().take(240).collect::<String>())),
                (d, _) => d.clone(),
            };
            widgets::section("Summary", &colors)
                .when(!summary.is_empty(), |this| {
                    this.child(div().font_weight(FontWeight::MEDIUM).child(summary.clone()))
                })
                .when_some(shown, |this, text| {
                    this.child(
                        div()
                            .text_size(u(12.0))
                            .text_color(colors.text_muted)
                            .whitespace_normal()
                            .child(text),
                    )
                })
                .when(long, |this| {
                    this.child(widgets::text_button(
                        "description-more",
                        if open { "Less" } else { "More" },
                        &colors,
                        {
                            let weak = cx.entity().downgrade();
                            move |_, _, cx| {
                                weak.update(cx, |this, cx| {
                                    this.details_state.description_open =
                                        !this.details_state.description_open;
                                    cx.notify();
                                })
                                .ok();
                            }
                        },
                    ))
                })
        });

        // Target.
        let target_section = alert.target.clone().map(|target| {
            let check = self.details_state.target.clone();
            let open = target.clone();
            let open_cluster = cluster.clone();
            let logs = target.clone();
            let logs_cluster = cluster.clone();
            let details = target.clone();
            let details_cluster = cluster.clone();
            let status = match &check {
                TargetCheck::Found(Some(status)) => {
                    let bad = matches!(
                        status.as_str(),
                        "CrashLoopBackOff"
                            | "Error"
                            | "Failed"
                            | "NotReady"
                            | "ImagePullBackOff"
                            | "ErrImagePull"
                    );
                    Some((status.clone(), if bad { colors.red } else { colors.green }))
                }
                TargetCheck::NotFound => Some(("not found".to_string(), colors.text_dim)),
                TargetCheck::Checking => Some(("checking…".to_string(), colors.text_faint)),
                _ => None,
            };
            let gone = check == TargetCheck::NotFound;
            widgets::section("Target", &colors)
                .child(
                    h_flex()
                        .gap(u(8.0))
                        .items_start()
                        .child(
                            Icon::new(target_icon(target.kind))
                                .size(13.0)
                                .color(colors.accent),
                        )
                        .child(div().flex_1().min_w_0().child(if gone {
                            div()
                                .font_family(fonts::MONO)
                                .text_size(u(12.0))
                                .text_color(colors.text_dim)
                                .line_through()
                                .whitespace_normal()
                                .child(target.label())
                                .into_any_element()
                        } else {
                            widgets::link(
                                "details-target",
                                target.label(),
                                &colors,
                                move |_, window, cx| {
                                    crate::actions::open_target(
                                        &open_cluster,
                                        &open,
                                        false,
                                        window,
                                        cx,
                                    )
                                },
                            )
                            .font_family(fonts::MONO)
                            .text_size(u(12.0))
                            .whitespace_normal()
                            .into_any_element()
                        }))
                        .when_some(status, |this, (status, color)| {
                            this.child(
                                h_flex()
                                    .flex_none()
                                    .gap(u(5.0))
                                    .text_size(u(12.0))
                                    .text_color(color)
                                    .child(kubyl_ui::StatusDot::new(color))
                                    .child(status),
                            )
                        }),
                )
                .when(!gone, |this| {
                    this.child(
                        h_flex()
                            .gap(u(14.0))
                            .text_size(u(12.0))
                            .when(target.kind == crate::model::TargetKind::Pod, |this| {
                                this.child(
                                    h_flex()
                                        .id("details-logs")
                                        .gap(u(5.0))
                                        .cursor_pointer()
                                        .hover(|s| s.text_color(colors.text))
                                        .text_color(colors.text_muted)
                                        .child(Icon::new(IconName::List).size(12.0))
                                        .child("Logs")
                                        .on_click(move |_, window, cx| {
                                            crate::actions::open_target(
                                                &logs_cluster,
                                                &logs,
                                                true,
                                                window,
                                                cx,
                                            )
                                        }),
                                )
                            })
                            .child(
                                div()
                                    .id("details-object")
                                    .cursor_pointer()
                                    .hover(|s| s.text_color(colors.text))
                                    .text_color(colors.text_muted)
                                    .child("Details")
                                    .on_click(move |_, window, cx| {
                                        crate::actions::open_target(
                                            &details_cluster,
                                            &details,
                                            false,
                                            window,
                                            cx,
                                        )
                                    }),
                            ),
                    )
                })
        });

        // Labels.
        let labels =
            widgets::section("Labels", &colors).child(h_flex().flex_wrap().gap(u(5.0)).children(
                alert.labels.iter().enumerate().map(|(i, (k, v))| {
                    let (name, value) = (k.clone(), v.clone());
                    div()
                        .id(("label-chip", i))
                        .cursor_pointer()
                        .child(widgets::label_chip(k, v, &colors))
                        .tooltip(|window, cx| {
                            gpui_component::tooltip::Tooltip::new(
                                "Click: filter · Alt-click: exclude",
                            )
                            .build(window, cx)
                        })
                        .on_click(
                            cx.listener(move |this, event: &gpui::ClickEvent, window, cx| {
                                let exclude = event.modifiers() == Modifiers::alt();
                                this.add_label_filter(&name, &value, exclude, window, cx);
                            }),
                        )
                }),
            ));

        // Annotations other than summary/description/runbook.
        let skip = ["summary", "description", "message", "runbook_url"];
        let annotations: Vec<(String, String)> = alert
            .annotations
            .iter()
            .filter(|(k, _)| !skip.contains(&k.as_str()))
            .map(|(k, v)| (k.clone(), v.clone()))
            .collect();
        let annotations_section = (!annotations.is_empty()).then(|| {
            widgets::section("Annotations", &colors).child(
                v_flex()
                    .gap(u(4.0))
                    .text_size(u(12.0))
                    .children(annotations.into_iter().map(|(k, v)| {
                        h_flex()
                            .items_start()
                            .gap(u(8.0))
                            .child(
                                div()
                                    .flex_none()
                                    .w(u(104.0))
                                    .text_color(colors.text_dim)
                                    .child(k),
                            )
                            .child(div().flex_1().min_w_0().whitespace_normal().child(v))
                    })),
            )
        });

        // Runbook and generator URL.
        let runbook = alert.runbook_url().map(str::to_string);
        let generator = alert.generator_url.clone();
        let links_section = (runbook.is_some() || generator.is_some()).then(|| {
            widgets::section("Runbook", &colors)
                .when_some(runbook.clone(), |this, url| {
                    let open = url.clone();
                    this.child(
                        div()
                            .id("runbook-link")
                            .text_size(u(12.0))
                            .font_family(fonts::MONO)
                            .text_color(colors.accent)
                            .whitespace_normal()
                            .cursor_pointer()
                            .hover(|s| s.underline())
                            .child(url)
                            .on_click(move |_, _, cx| widgets::open_url(&open, cx)),
                    )
                })
                .when_some(generator.clone(), |this, url| {
                    let open = url.clone();
                    let cluster = cluster.clone();
                    this.child(
                        h_flex()
                            .gap(u(12.0))
                            .text_size(u(12.0))
                            .child(widgets::text_button(
                                "open-generator",
                                "Open in Prometheus",
                                &colors,
                                move |_, window, cx| {
                                    crate::actions::open_generator(&cluster, &open, window, cx)
                                },
                            ))
                            .child(
                                div()
                                    .truncate()
                                    .text_color(colors.text_dim)
                                    .font_family(fonts::MONO)
                                    .text_size(u(11.0))
                                    .child(url),
                            ),
                    )
                })
        });

        // Routing: silenced and inhibited by.
        let routing_section = (!alert.silenced_by.is_empty() || !alert.inhibited_by.is_empty())
            .then(|| {
                let mut section = widgets::section("Routing", &colors);
                for id in &alert.silenced_by {
                    let silence = silences.iter().find(|s| &s.id == id).cloned();
                    let select = id.clone();
                    section = section.child(
                        v_flex()
                            .gap(u(2.0))
                            .text_size(u(12.0))
                            .child(
                                h_flex()
                                    .gap(u(6.0))
                                    .child(
                                        Icon::new(IconName::BellOff)
                                            .size(12.0)
                                            .color(colors.text_dim),
                                    )
                                    .child("Silenced by")
                                    .child(
                                        widgets::link(
                                            SharedString::from(format!("silence-{id}")),
                                            format!(
                                                "silence {}",
                                                id.chars().take(8).collect::<String>()
                                            ),
                                            &colors,
                                            {
                                                let weak = cx.entity().downgrade();
                                                move |_, _, cx| {
                                                    let select = select.clone();
                                                    weak.update(cx, |this, cx| {
                                                        this.tab = Tab::Silences;
                                                        this.selected_silence = Some(select);
                                                        cx.notify();
                                                    })
                                                    .ok();
                                                }
                                            },
                                        )
                                        .font_family(fonts::MONO),
                                    ),
                            )
                            .when_some(silence, |this, silence| {
                                this.child(
                                    div()
                                        .pl(u(18.0))
                                        .text_color(colors.text_muted)
                                        .whitespace_normal()
                                        .child(format!(
                                            "“{}” · {} · ends {}",
                                            silence.comment,
                                            silence.created_by,
                                            silence
                                                .ends_at
                                                .map(|t| format!(
                                                    "in {}",
                                                    widgets::short_duration(
                                                        t.duration_since(now).as_secs()
                                                    )
                                                ))
                                                .unwrap_or_default()
                                        )),
                                )
                            }),
                    );
                }
                for fingerprint in &alert.inhibited_by {
                    let by = self
                        .alerts
                        .iter()
                        .find(|a| &a.fingerprint == fingerprint)
                        .cloned();
                    let select = fingerprint.clone();
                    section = section.child(
                        h_flex()
                            .gap(u(6.0))
                            .text_size(u(12.0))
                            .child(
                                Icon::new(IconName::EyeOff)
                                    .size(12.0)
                                    .color(colors.text_dim),
                            )
                            .child("Inhibited by")
                            .child(
                                widgets::link(
                                    SharedString::from(format!("inhibitor-{fingerprint}")),
                                    by.map(|a| a.name).unwrap_or_else(|| fingerprint.clone()),
                                    &colors,
                                    {
                                        let weak = cx.entity().downgrade();
                                        move |_, _, cx| {
                                            let select = select.clone();
                                            weak.update(cx, |this, cx| {
                                                this.selected = Some(select);
                                                this.filters.show_suppressed = true;
                                                this.rebuild(cx);
                                            })
                                            .ok();
                                        }
                                    },
                                )
                                .font_family(fonts::MONO),
                            ),
                    );
                }
                section
            });

        // Rule.
        let rule_section = rule.map(|rule| self.render_rule_section(&cluster, &rule, cx));

        // Timeline.
        let timeline_section = match &self.details_state.timeline {
            Timeline::Ready { slots, .. } => Some(
                widgets::section("Last 24 h", &colors)
                    .child(
                        h_flex()
                            .h(u(12.0))
                            .w_full()
                            .children(slots.iter().map(|slot| {
                                let color = match slot {
                                    Some(AlertState::Firing) => colors.red,
                                    Some(AlertState::Pending) => colors.yellow,
                                    _ => colors.bar_track,
                                };
                                div().flex_1().h_full().bg(color)
                            })),
                    )
                    .child(
                        h_flex()
                            .text_size(u(10.5))
                            .text_color(colors.text_dim)
                            .child("24 h ago")
                            .child(div().flex_1())
                            .child("now"),
                    ),
            ),
            Timeline::Loading => Some(
                widgets::section("Last 24 h", &colors).child(
                    div()
                        .text_size(u(12.0))
                        .text_color(colors.text_dim)
                        .child("Loading…"),
                ),
            ),
            _ => None,
        };

        let close = cx.listener(|this, _, _, cx| {
            this.details_open = false;
            cx.notify();
        });
        v_flex()
            .flex_none()
            .w(u(350.0))
            .h_full()
            .border_l_1()
            .border_color(colors.border)
            .bg(colors.panel)
            .child(
                h_flex()
                    .flex_none()
                    .h(u(34.0))
                    .px(u(14.0))
                    .border_b_1()
                    .border_color(colors.border_variant)
                    .child(
                        div()
                            .flex_1()
                            .font_weight(FontWeight::SEMIBOLD)
                            .child("Alert"),
                    )
                    .child(
                        kubyl_ui::IconButton::new("details-close", IconName::X)
                            .icon_size(12.0)
                            .on_click(close),
                    ),
            )
            .child(
                v_flex()
                    .id("alert-details-scroll")
                    .flex_1()
                    .min_h_0()
                    .overflow_y_scroll()
                    .child(title)
                    .child(times)
                    .children(summary_section)
                    .children(target_section)
                    .child(labels)
                    .children(annotations_section)
                    .children(links_section)
                    .children(routing_section)
                    .children(rule_section)
                    .children(timeline_section),
            )
            .into_any_element()
    }

    fn render_detail_actions(
        &self,
        entry: &Entry,
        can_silence: bool,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let colors = cx.colors().clone();
        let silence_entry = entry.clone();
        let ack_entry = entry.clone();
        let labels = matchers::format(&entry.alert.matchers());
        let amtool = format!(
            "amtool alert query {}",
            matchers::amtool_args(&entry.alert.matchers())
        );
        h_flex()
            .gap(u(8.0))
            .when(can_silence, |this| {
                this.child(
                    Button::new("details-silence")
                        .primary()
                        .icon(IconName::BellOff)
                        .label("Silence…")
                        .on_click(move |_, window, cx| {
                            crate::silence::open_for_alert(
                                &silence_entry.cluster,
                                &silence_entry.alert,
                                window,
                                cx,
                            )
                        }),
                )
                .child(Button::new("details-ack").label("Acknowledge").on_click(
                    move |_, window, cx| {
                        crate::silence::acknowledge(
                            &ack_entry.cluster,
                            &ack_entry.alert,
                            window,
                            cx,
                        )
                    },
                ))
            })
            .child(div().flex_1())
            .child(
                MenuButton::new("details-copy")
                    .ghost()
                    .compact()
                    .child(
                        h_flex()
                            .gap(u(4.0))
                            .text_size(u(12.0))
                            .text_color(colors.text_muted)
                            .child(Icon::new(IconName::Copy).size(12.0))
                            .child("Copy")
                            .child(Icon::new(IconName::ChevronDown).size(11.0)),
                    )
                    .dropdown_menu(move |menu, _, _| {
                        let labels = labels.clone();
                        let amtool = amtool.clone();
                        menu.max_h(px(200.0))
                            .item(PopupMenuItem::new("Copy labels as matchers").on_click(
                                move |_, _, cx| widgets::copy(labels.clone(), "the labels", cx),
                            ))
                            .item(PopupMenuItem::new("Copy as amtool filter").on_click(
                                move |_, _, cx| {
                                    widgets::copy(amtool.clone(), "the amtool command", cx)
                                },
                            ))
                    }),
            )
            .into_any_element()
    }

    /// The rule's section (alert details and the Rules tab share it).
    pub(crate) fn render_rule_section(
        &mut self,
        cluster: &ClusterId,
        rule: &Rule,
        cx: &mut Context<Self>,
    ) -> gpui::Div {
        let colors: Colors = cx.colors().clone();
        let now = Timestamp::now();
        let object = self.rule_objects.find(cluster, &rule.group, &rule.name, cx);
        let expression = rule.query.clone();
        let copy_expression = expression.clone();
        let edit_cluster = cluster.clone();
        let health_color = if rule.failing() {
            colors.red
        } else {
            colors.green
        };
        let mut kv: Vec<(&'static str, AnyElement)> = vec![
            ("Group", div().child(rule.group.clone()).into_any_element()),
            (
                "for",
                div()
                    .font_family(fonts::MONO)
                    .child(if rule.duration > 0.0 {
                        widgets::short_duration(rule.duration as i64)
                    } else {
                        "—".into()
                    })
                    .into_any_element(),
            ),
            (
                "keep_firing_for",
                div()
                    .font_family(fonts::MONO)
                    .child(if rule.keep_firing_for > 0.0 {
                        widgets::short_duration(rule.keep_firing_for as i64)
                    } else {
                        "—".into()
                    })
                    .into_any_element(),
            ),
            (
                "Health",
                div()
                    .text_color(health_color)
                    .child(rule.health.clone())
                    .into_any_element(),
            ),
        ];
        if let Some(at) = rule.last_evaluation {
            kv.push((
                "Evaluated",
                div()
                    .child(format!(
                        "{} ago · took {}",
                        widgets::ago(Some(at), now),
                        eval_time(rule.evaluation_time)
                    ))
                    .into_any_element(),
            ));
        }
        if let Some((ns, name)) = object.clone() {
            let open = (ns.clone(), name.clone());
            kv.push((
                "Defined in",
                widgets::link(
                    "rule-object",
                    format!("{ns}/{name}"),
                    &colors,
                    move |_, window, cx| {
                        crate::actions::edit_prometheus_rule(
                            &edit_cluster,
                            &open.0,
                            &open.1,
                            window,
                            cx,
                        )
                    },
                )
                .font_family(fonts::MONO)
                .into_any_element(),
            ));
        }
        widgets::section(format!("Rule · {}", rule.group), &colors)
            .when_some(rule.last_error.clone(), |this, error| {
                this.child(
                    div()
                        .font_family(fonts::MONO)
                        .text_size(u(11.5))
                        .text_color(colors.red)
                        .whitespace_normal()
                        .child(error),
                )
            })
            .child(
                div()
                    .p(u(8.0))
                    .rounded(u(5.0))
                    .bg(colors.background)
                    .font_family(fonts::MONO)
                    .text_size(u(11.5))
                    .whitespace_normal()
                    .child(expression),
            )
            .child(
                h_flex()
                    .gap(u(14.0))
                    .text_size(u(12.0))
                    .child(
                        h_flex()
                            .id("copy-expression")
                            .gap(u(5.0))
                            .cursor_pointer()
                            .text_color(colors.text_muted)
                            .hover(|s| s.text_color(colors.text))
                            .child(Icon::new(IconName::Copy).size(12.0))
                            .child("Copy")
                            .on_click(move |_, _, cx| {
                                widgets::copy(copy_expression.clone(), "the expression", cx)
                            }),
                    )
                    .when_some(object, |this, (ns, name)| {
                        let cluster = cluster.clone();
                        this.child(
                            h_flex()
                                .id("edit-rule")
                                .gap(u(5.0))
                                .cursor_pointer()
                                .text_color(colors.text_muted)
                                .hover(|s| s.text_color(colors.text))
                                .child(Icon::new(IconName::Code).size(12.0))
                                .child("Edit PrometheusRule")
                                .on_click(move |_, window, cx| {
                                    crate::actions::edit_prometheus_rule(
                                        &cluster, &ns, &name, window, cx,
                                    )
                                }),
                        )
                    }),
            )
            .child(widgets::kv(kv, &colors))
    }
}

/// `4ms`, `1.2s`.
pub(crate) fn eval_time(seconds: f64) -> String {
    if seconds < 1.0 {
        format!("{}ms", (seconds * 1000.0).round().max(1.0) as i64)
    } else {
        format!("{seconds:.1}s")
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn timelines_follow_the_alert_series() {
        let labels: BTreeMap<String, String> = [
            ("alertname", "KubePodCrashLooping"),
            ("namespace", "payments"),
            ("pod", "a"),
            ("prometheus", "monitoring/kube-prom"),
        ]
        .iter()
        .map(|(k, v)| (k.to_string(), v.to_string()))
        .collect();
        let series = |pod: &str, state: &str, times: &[f64]| RangeSeries {
            labels: [
                ("alertname", "KubePodCrashLooping"),
                ("namespace", "payments"),
                ("pod", pod),
                ("alertstate", state),
            ]
            .iter()
            .map(|(k, v)| (k.to_string(), v.to_string()))
            .collect(),
            values: times.iter().map(|t| (*t, 1.0)).collect(),
        };
        let (slots, fired) = timeline(
            &[
                series("a", "pending", &[0.0, 300.0]),
                series("a", "firing", &[600.0, 900.0, 1800.0]),
                series("b", "firing", &[0.0, 300.0, 600.0]),
            ],
            &labels,
            0.0,
            1800.0,
            300.0,
        );
        assert_eq!(slots.len(), 7);
        assert_eq!(slots[0], Some(AlertState::Pending));
        assert_eq!(slots[2], Some(AlertState::Firing));
        assert_eq!(slots[4], None);
        assert_eq!(fired, 2, "fired, stopped, fired again");
    }

    #[test]
    fn object_paths() {
        let pod = Target {
            kind: crate::model::TargetKind::Pod,
            namespace: Some("payments".into()),
            name: "gw-1".into(),
            container: None,
        };
        assert_eq!(object_path(&pod), "/api/v1/namespaces/payments/pods/gw-1");
        let node = Target {
            kind: crate::model::TargetKind::Node,
            namespace: None,
            name: "n1".into(),
            container: None,
        };
        assert_eq!(object_path(&node), "/api/v1/nodes/n1");
        assert_eq!(eval_time(0.0042), "4ms");
    }
}
