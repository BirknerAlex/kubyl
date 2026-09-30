//! Overview: build and runtime info, what the targets and rules look like, and the TSDB status
//! (head statistics and the heaviest metrics and labels).

use gpui::{AnyElement, Context, FontWeight, IntoElement, SharedString, div, prelude::*};
use jiff::Timestamp;
use kubyl_metrics::prometheus::PromError;
use kubyl_ui::{ActiveColors, Colors, fonts, h_flex, u, v_flex};

use super::{PrometheusView, widgets};
use crate::model::{Health, Stat, Tsdb};
use crate::service::Instance;

const TOP: usize = 10;

impl PrometheusView {
    pub(super) fn render_overview(
        &mut self,
        instance: &Instance,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let colors = cx.colors().clone();
        let Some(overview) = self.overview.data.clone() else {
            return match &self.overview.error {
                Some(error) => {
                    widgets::empty(format!("Couldn't read the status: {error}"), &colors)
                }
                None => widgets::empty("Loading status…", &colors),
            };
        };
        let now = Timestamp::now();
        // KPI tiles: what needs a look first.
        let mut tiles: Vec<AnyElement> = Vec::new();
        if let Some(targets) = &self.targets.data {
            let down = targets.count(Health::Down);
            tiles.push(tile(
                "Targets up",
                format!("{}/{}", targets.count(Health::Up), targets.active.len()),
                (down > 0).then(|| format!("{down} down")),
                down > 0,
                &colors,
            ));
        }
        if let Some(groups) = &self.rules.data {
            let rules: Vec<_> = groups.iter().flat_map(|g| &g.rules).collect();
            let failing = rules.iter().filter(|r| r.failing()).count();
            let firing = rules.iter().filter(|r| r.firing()).count();
            tiles.push(tile(
                "Rules",
                rules.len().to_string(),
                (failing > 0).then(|| format!("{failing} failing")),
                failing > 0,
                &colors,
            ));
            tiles.push(tile(
                "Alerts firing",
                firing.to_string(),
                None,
                firing > 0,
                &colors,
            ));
        }
        if let Some(Ok(tsdb)) = &overview.tsdb {
            tiles.push(tile(
                "Head series",
                tsdb.series.map_or("—".into(), widgets::count),
                None,
                false,
                &colors,
            ));
        }

        // Server.
        let mut server: Vec<(&'static str, AnyElement)> = vec![(
            "Server",
            div()
                .child(format!("{} · {}", instance.kind.label(), instance.label()))
                .into_any_element(),
        )];
        {
            {
                if let Some(Ok(build)) = &overview.build {
                    let text = |v: &Option<String>| {
                        div()
                            .child(v.clone().unwrap_or_else(|| "—".into()))
                            .into_any_element()
                    };
                    server.push(("Version", text(&build.version)));
                    if let Some(revision) = &build.revision {
                        server.push((
                            "Revision",
                            div()
                                .font_family(fonts::MONO)
                                .child(revision.chars().take(12).collect::<String>())
                                .into_any_element(),
                        ));
                    }
                    server.push(("Go version", text(&build.go_version)));
                    server.push(("Build date", text(&build.build_date)));
                }
                match &overview.runtime {
                    Some(Ok(runtime)) => {
                        if let Some(start) = runtime.start_time {
                            server.push((
                                "Uptime",
                                div()
                                    .child(format!(
                                        "{} (since {})",
                                        widgets::short_duration(
                                            now.duration_since(start).as_secs()
                                        ),
                                        widgets::local_time(start)
                                    ))
                                    .into_any_element(),
                            ));
                        }
                        if let Some(retention) = &runtime.storage_retention {
                            server.push((
                                "Retention",
                                div().child(retention.clone()).into_any_element(),
                            ));
                        }
                        if let Some(ok) = runtime.reload_config_success {
                            server.push((
                                "Last config reload",
                                div()
                                    .text_color(if ok { colors.green } else { colors.red })
                                    .child(if ok { "successful" } else { "failed" })
                                    .into_any_element(),
                            ));
                        }
                        if let Some(corruptions) = runtime.corruption_count {
                            server.push((
                                "WAL corruptions",
                                div()
                                    .text_color(if corruptions > 0 {
                                        colors.red
                                    } else {
                                        colors.text
                                    })
                                    .child(corruptions.to_string())
                                    .into_any_element(),
                            ));
                        }
                        if let Some(goroutines) = runtime.goroutines {
                            server.push((
                                "Goroutines",
                                div().child(goroutines.to_string()).into_any_element(),
                            ));
                        }
                    }
                    Some(Err(err)) => {
                        server.push(("Runtime", unavailable(err, &colors).into_any_element()))
                    }
                    None => {}
                }
            }
        }
        let server_section =
            widgets::section("Server", &colors).child(widgets::kv(server, &colors));

        // TSDB.
        let tsdb_sections: Vec<AnyElement> = match &overview.tsdb {
            Some(Ok(tsdb)) => tsdb_sections(tsdb, &colors),
            Some(Err(err)) => vec![
                widgets::section("TSDB status", &colors)
                    .child(unavailable(err, &colors))
                    .into_any_element(),
            ],
            None => Vec::new(),
        };
        let error = self
            .overview
            .error
            .as_ref()
            .map(|e| Self::error_banner(e, &colors));
        v_flex()
            .id("prometheus-overview")
            .size_full()
            .overflow_y_scroll()
            .children(error)
            .when(!tiles.is_empty(), |this| {
                this.child(
                    h_flex()
                        .flex_none()
                        .flex_wrap()
                        .gap(u(10.0))
                        .px(u(14.0))
                        .py(u(12.0))
                        .border_b_1()
                        .border_color(colors.border_variant)
                        .children(tiles),
                )
            })
            .child(
                h_flex()
                    .flex_wrap()
                    .items_start()
                    .child(div().flex_1().min_w(u(380.0)).child(server_section))
                    .children(
                        tsdb_sections
                            .into_iter()
                            .map(|s| div().flex_1().min_w(u(380.0)).child(s)),
                    ),
            )
            .into_any_element()
    }
}

/// `Not available from this server (…)`: Thanos Query has no TSDB status, for one.
fn unavailable(err: &PromError, colors: &Colors) -> gpui::Div {
    div()
        .text_size(u(12.0))
        .text_color(colors.text_dim)
        .child(match err {
            PromError::Http(404 | 405, _) => "Not available from this server.".to_string(),
            err => format!("Not available from this server ({err})."),
        })
}

fn tile(
    title: &str,
    value: String,
    note: Option<String>,
    bad: bool,
    colors: &Colors,
) -> AnyElement {
    v_flex()
        .flex_none()
        .min_w(u(140.0))
        .h(u(92.0))
        .px(u(12.0))
        .py(u(8.0))
        .gap(u(2.0))
        .rounded(u(6.0))
        .border_1()
        .border_color(colors.border_variant)
        .bg(colors.chip_background)
        .child(
            div()
                .text_size(u(11.0))
                .text_color(colors.text_dim)
                .child(title.to_uppercase()),
        )
        .child(
            div()
                .text_size(u(20.0))
                .font_weight(FontWeight::SEMIBOLD)
                .text_color(if bad { colors.red } else { colors.text })
                .child(value),
        )
        .child(
            div()
                .h(u(16.0))
                .text_size(u(11.5))
                .text_color(colors.red)
                .child(note.unwrap_or_default()),
        )
        .into_any_element()
}

fn tsdb_sections(tsdb: &Tsdb, colors: &Colors) -> Vec<AnyElement> {
    let number = |v: Option<u64>| {
        div()
            .child(v.map_or("—".into(), widgets::count))
            .into_any_element()
    };
    let time = |ms: Option<i64>| {
        div()
            .child(
                ms.and_then(|ms| Timestamp::from_millisecond(ms).ok())
                    .map_or("—".into(), widgets::local_time),
            )
            .into_any_element()
    };
    let mut sections = vec![
        widgets::section("Head block", colors)
            .child(widgets::kv(
                vec![
                    ("Series", number(tsdb.series)),
                    ("Chunks", number(tsdb.chunks)),
                    ("Label pairs", number(tsdb.label_pairs)),
                    ("Oldest sample", time(tsdb.min_time)),
                    ("Newest sample", time(tsdb.max_time)),
                ],
                colors,
            ))
            .into_any_element(),
    ];
    for (title, stats, format) in [
        (
            "Series by metric name",
            &tsdb.series_by_metric,
            widgets::count as fn(u64) -> String,
        ),
        (
            "Label names by value count",
            &tsdb.label_value_counts,
            widgets::count,
        ),
        (
            "Series by label value pair",
            &tsdb.series_by_pair,
            widgets::count,
        ),
        (
            "Memory by label name",
            &tsdb.memory_by_label,
            widgets::bytes,
        ),
    ] {
        if !stats.is_empty() {
            sections.push(top(title, stats, format, colors));
        }
    }
    sections
}

/// The heaviest entries with a bar relative to the first.
fn top(
    title: &'static str,
    stats: &[Stat],
    format: fn(u64) -> String,
    colors: &Colors,
) -> AnyElement {
    let mut stats: Vec<&Stat> = stats.iter().collect();
    stats.sort_by(|a, b| b.value.cmp(&a.value).then_with(|| a.name.cmp(&b.name)));
    let max = stats.first().map_or(1, |s| s.value.max(1)) as f32;
    widgets::section(format!("{title} (top {TOP})"), colors)
        .children(stats.into_iter().take(TOP).map(|stat| {
            let share = (stat.value as f32 / max).clamp(0.0, 1.0);
            v_flex()
                .gap(u(3.0))
                .child(
                    h_flex()
                        .gap(u(8.0))
                        .text_size(u(12.0))
                        .child(
                            div()
                                .flex_1()
                                .min_w_0()
                                .truncate()
                                .font_family(fonts::MONO)
                                .child(SharedString::from(stat.name.clone())),
                        )
                        .child(
                            div()
                                .flex_none()
                                .font_family(fonts::MONO)
                                .text_color(colors.text_muted)
                                .child(format(stat.value)),
                        ),
                )
                .child(
                    div()
                        .h(u(3.0))
                        .w_full()
                        .rounded(u(2.0))
                        .bg(colors.border_variant)
                        .child(
                            div()
                                .h_full()
                                .w(gpui::relative(share))
                                .rounded(u(2.0))
                                .bg(colors.accent.opacity(0.7)),
                        ),
                )
        }))
        .into_any_element()
}
