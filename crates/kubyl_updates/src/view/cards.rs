//! The cards of the Updates tab: notes, progress, version and channel, the update path,
//! pre-flight checks, cluster operators, pools, add-ons, history and docs.

use gpui::{
    AnyElement, App, ClipboardItem, Context, FontWeight, Hsla, IntoElement, PathBuilder,
    SharedString, Window, canvas, div, point, prelude::*, px,
};
use gpui_component::button::Button as MenuButton;
use gpui_component::menu::{DropdownMenu as _, PopupMenuItem};
use kubyl_core::actions::OpenView;
use kubyl_core::{ClusterId, Notification, NotificationCenter, ResourceRef, ViewKind, ViewRequest};
use kubyl_ui::{ActiveColors, Button, Colors, Icon, IconName, fonts, h_flex, u, v_flex};

use crate::check::{Check, CheckStatus, Counts, Fix};
use crate::model::{
    Component, PoolKind, PoolState, Progress, ProviderKind, Scope, Status, Target, TargetKind,
};
use crate::service::Updates;
use crate::view::UpdatesView;
use crate::view::widgets::{
    self, ago, bar, card, card_header, cell, date, kv, link, mono, row_button, status_icon,
    table_header, table_row, took,
};

/// Height of one target row of the update path.
const ROW: f32 = 30.0;
/// Where the target column starts (the current version sits left of it).
const TARGETS_X: f32 = 160.0;
/// The center of the current version's node.
const CURRENT_X: f32 = 116.0;

/// A note or warning across the page (credentials missing, read-only, a stale read).
pub fn banner(
    warning: bool,
    title: String,
    text: String,
    command: Option<String>,
    url: Option<String>,
    id: impl Into<gpui::ElementId>,
    colors: &Colors,
) -> AnyElement {
    let color = if warning {
        colors.yellow
    } else {
        colors.accent
    };
    let id: gpui::ElementId = id.into();
    h_flex()
        .id(id.clone())
        .items_start()
        .gap(u(10.0))
        .px(u(14.0))
        .py(u(10.0))
        .rounded(u(8.0))
        .border_1()
        .border_color(color.opacity(0.5))
        .bg(color.opacity(0.06))
        .child(
            Icon::new(if warning {
                IconName::TriangleAlert
            } else {
                IconName::Info
            })
            .size(15.0)
            .color(color),
        )
        .child(
            v_flex()
                .flex_1()
                .min_w_0()
                .gap(u(2.0))
                .child(div().font_weight(FontWeight::MEDIUM).child(title))
                .child(
                    div()
                        .text_size(u(12.0))
                        .text_color(colors.text_muted)
                        .child(text),
                )
                .when_some(command.clone(), |this, command| {
                    this.child(
                        div()
                            .mt(u(4.0))
                            .font_family(fonts::MONO)
                            .text_size(u(11.5))
                            .child(command),
                    )
                }),
        )
        .when_some(command, |this, command| {
            this.child(
                row_button(
                    gpui::ElementId::NamedChild(std::sync::Arc::new(id.clone()), "copy".into()),
                    "Copy command",
                    colors,
                )
                .on_click(move |_, _, cx| {
                    cx.write_to_clipboard(ClipboardItem::new_string(command.clone()));
                    NotificationCenter::push(cx, Notification::info("Copied the command."));
                }),
            )
        })
        .when_some(url, |this, url| {
            this.child(
                row_button(
                    gpui::ElementId::NamedChild(std::sync::Arc::new(id.clone()), "docs".into()),
                    "Docs",
                    colors,
                )
                .on_click(move |_, window, cx| widgets::open_url(&url, window, cx)),
            )
        })
        .into_any_element()
}

/// The running update: target, percent, the provider's message, counts.
pub fn progress(status: &Status, progress: &Progress, colors: &Colors) -> AnyElement {
    let updated = status.components.iter().filter(|c| c.updated).count();
    let nodes: (usize, usize) = status.pools.iter().fold((0, 0), |acc, p| {
        (
            acc.0 + p.updated.unwrap_or_default(),
            acc.1 + p.nodes.unwrap_or_default(),
        )
    });
    let percent = progress.percent.or_else(|| {
        (!status.components.is_empty())
            .then(|| updated as f32 * 100.0 / status.components.len() as f32)
    });
    let mut line = format!("Updating to {}", progress.target);
    if let Some(p) = percent {
        line.push_str(&format!(" · {p:.0}%"));
    }
    if let Some(started) = progress.started {
        line.push_str(&format!(" · started {}", ago(started)));
    }
    let mut counts = Vec::new();
    if !status.components.is_empty() {
        counts.push(format!(
            "Cluster operators {updated} / {}",
            status.components.len()
        ));
    }
    if nodes.1 > 0 {
        counts.push(format!("Nodes {} / {}", nodes.0, nodes.1));
    }
    card(colors)
        .border_color(if progress.failing.is_some() {
            colors.red
        } else {
            colors.accent.opacity(0.6)
        })
        .child(
            v_flex()
                .px(u(16.0))
                .py(u(12.0))
                .gap(u(8.0))
                .child(
                    h_flex()
                        .gap(u(8.0))
                        .child(
                            Icon::new(IconName::RefreshCw)
                                .size(14.0)
                                .color(colors.accent),
                        )
                        .child(div().font_weight(FontWeight::SEMIBOLD).child(line))
                        .child(div().flex_1())
                        .child(
                            mono(format!("{} → {}", status.current.version, progress.target))
                                .text_color(colors.text_dim),
                        ),
                )
                .child(widgets::wide_bar(
                    percent.unwrap_or(0.0),
                    colors.accent,
                    colors,
                ))
                .when(!progress.message.is_empty(), |this| {
                    this.child(
                        h_flex()
                            .gap(u(8.0))
                            .text_size(u(12.0))
                            .child(div().text_color(colors.text_dim).child("Progressing"))
                            .child(mono(progress.message.clone()).min_w_0().truncate()),
                    )
                })
                .when_some(progress.failing.clone(), |this, failing| {
                    this.child(
                        h_flex()
                            .gap(u(6.0))
                            .text_size(u(12.0))
                            .text_color(colors.red)
                            .child(Icon::new(IconName::CircleX).size(12.0).color(colors.red))
                            .child(failing),
                    )
                })
                .child(
                    h_flex()
                        .gap(u(16.0))
                        .text_size(u(12.0))
                        .text_color(colors.text_muted)
                        .children(counts)
                        .child(div().flex_1())
                        .child(
                            div()
                                .text_color(colors.text_dim)
                                .child("Closing Kubyl doesn't stop the update."),
                        ),
                ),
        )
        .into_any_element()
}

fn condition_line(ok: bool, text: String, colors: &Colors) -> impl IntoElement {
    let (icon, color) = if ok {
        (IconName::CircleCheck, colors.green)
    } else {
        (IconName::TriangleAlert, colors.yellow)
    };
    h_flex()
        .gap(u(6.0))
        .text_size(u(12.0))
        .text_color(color)
        .child(Icon::new(icon).size(12.0).color(color))
        .child(div().min_w_0().child(text))
}

fn small_label(text: &'static str, colors: &Colors) -> impl IntoElement {
    div()
        .text_size(u(12.0))
        .text_color(colors.text_dim)
        .child(text)
}

/// Current version, platform, cluster id, last update, conditions, channel.
pub fn version_card(
    view: &UpdatesView,
    status: &Status,
    read_only: bool,
    _: &mut Window,
    cx: &mut Context<UpdatesView>,
) -> AnyElement {
    let colors = cx.colors().clone();
    let current = &status.current;
    let mut sub = Vec::new();
    if let Some(kube) = &current.kubernetes {
        sub.push(format!(
            "Kubernetes {}",
            kube.split(['-', '+']).next().unwrap_or(kube)
        ));
    }
    if let Some(platform) = &current.platform {
        sub.push(format!("platform {platform}"));
    }
    let mut body = v_flex().gap(u(12.0)).child(
        v_flex()
            .child(small_label("Current version", &colors))
            .child(
                div()
                    .font_family(fonts::MONO)
                    .text_size(u(24.0))
                    .font_weight(FontWeight::MEDIUM)
                    .child(current.version.clone()),
            )
            .when(!sub.is_empty(), |this| {
                this.child(
                    div()
                        .text_size(u(11.5))
                        .text_color(colors.text_dim)
                        .child(sub.join(" · ")),
                )
            })
            .when_some(current.cluster_id.clone(), |this, id| {
                // UUIDs are shortened; ARNs and names aren't.
                let short = if id.len() > 13 && !id.contains(':') && !id.contains('/') {
                    format!("{}…{}", &id[..8], &id[id.len() - 4..])
                } else {
                    id.clone()
                };
                this.child(
                    h_flex()
                        .gap(u(6.0))
                        .text_size(u(11.5))
                        .text_color(colors.text_dim)
                        .child("Cluster ID")
                        .child(mono(short).text_size(u(11.5)).min_w_0().truncate())
                        .child(
                            div()
                                .id("copy-cluster-id")
                                .cursor_pointer()
                                .child(Icon::new(IconName::Copy).size(11.0).color(colors.text_dim))
                                .on_click(move |_, _, cx| {
                                    cx.write_to_clipboard(ClipboardItem::new_string(id.clone()));
                                    NotificationCenter::push(
                                        cx,
                                        Notification::info("Copied the cluster ID."),
                                    );
                                }),
                        ),
                )
            })
            .when_some(current.support.clone(), |this, support| {
                this.child(
                    div()
                        .text_size(u(11.5))
                        .text_color(if support.warning {
                            colors.yellow
                        } else {
                            colors.text_dim
                        })
                        .child(support.text),
                )
            }),
    );
    if let Some(last) = status.history.iter().find(|h| h.completed()) {
        let previous = status
            .history
            .iter()
            .skip_while(|h| h.version != last.version)
            .nth(1);
        let mut lines = v_flex()
            .gap(u(1.0))
            .child(small_label("Last update", &colors));
        lines = lines.child(mono(match previous {
            Some(p) => format!("{} → {}", p.version, last.version),
            None => last.version.clone(),
        }));
        if let (Some(started), Some(completed)) = (last.started, last.completed) {
            lines = lines.child(div().text_size(u(11.5)).text_color(colors.text_dim).child(
                format!("{} · took {}", date(completed), took(started, completed)),
            ));
        }
        body = body.child(lines);
    }
    let shown: Vec<_> = current
        .conditions
        .iter()
        .filter(|c| {
            matches!(
                c.kind.as_str(),
                "Available" | "Failing" | "Upgradeable" | "RetrievedUpdates"
            )
        })
        .filter(|c| match c.kind.as_str() {
            // Only what's worth a line: Available, and the others when they're not fine.
            "Available" => true,
            "Failing" => c.status == Some(true),
            _ => c.status == Some(false),
        })
        .collect();
    if !shown.is_empty() {
        let mut lines = v_flex().gap(u(3.0)).child(small_label("Status", &colors));
        for c in shown {
            let ok = match c.kind.as_str() {
                "Available" => c.status == Some(true),
                "Failing" => false,
                _ => false,
            };
            let text = match (c.kind.as_str(), &c.reason) {
                ("Available", _) if ok => "Available".to_string(),
                (kind, Some(reason)) => format!("{kind}: {} · {reason}", status_word(c.status)),
                (kind, None) => format!("{kind}: {}", status_word(c.status)),
            };
            lines = lines.child(condition_line(ok, text, &colors));
        }
        body = body.child(lines);
    }
    // Channel: a menu where the provider can change it.
    if current.channel.is_some() || !current.channels.is_empty() {
        let label = current.channel.clone().unwrap_or_else(|| "none".into());
        let can_change = status.writes.channel && !read_only && !current.channels.is_empty();
        let control: AnyElement = if can_change {
            let weak = cx.entity().downgrade();
            let channels = current.channels.clone();
            let active = current.channel.clone();
            MenuButton::new("updates-channel")
                .outline()
                .compact()
                .w_full()
                .child(
                    h_flex()
                        .w_full()
                        .gap(u(6.0))
                        .font_family(fonts::MONO)
                        .text_size(u(12.0))
                        .child(div().flex_1().child(label))
                        .child(Icon::new(IconName::ChevronDown).size(11.0)),
                )
                .dropdown_menu(move |mut menu, _, _| {
                    for channel in &channels {
                        let weak = weak.clone();
                        let value = channel.clone();
                        menu = menu.item(
                            PopupMenuItem::new(channel.clone())
                                .checked(Some(channel) == active.as_ref())
                                .on_click(move |_, window, cx| {
                                    let value = value.clone();
                                    weak.update(cx, |view, cx| {
                                        view.confirm(
                                            Scope::Channel(value.clone()),
                                            String::new(),
                                            window,
                                            cx,
                                        )
                                    })
                                    .ok();
                                }),
                        );
                    }
                    menu
                })
                .into_any_element()
        } else {
            mono(label).into_any_element()
        };
        body = body.child(
            v_flex()
                .gap(u(4.0))
                .child(small_label("Channel", &colors))
                .child(control)
                .when(can_change, |this| {
                    this.child(
                        div()
                            .text_size(u(11.0))
                            .text_color(colors.text_faint)
                            .child("Changing it shows a summary first."),
                    )
                }),
        );
    }
    let _ = view;
    card(&colors)
        .w(u(260.0))
        .px(u(16.0))
        .py(u(14.0))
        .child(body)
        .into_any_element()
}

fn status_word(status: Option<bool>) -> &'static str {
    match status {
        Some(true) => "True",
        Some(false) => "False",
        None => "Unknown",
    }
}

fn target_color(kind: TargetKind, colors: &Colors) -> Hsla {
    match kind {
        TargetKind::Recommended => colors.accent,
        TargetKind::Available => colors.text_dim,
        TargetKind::Conditional => colors.yellow,
        TargetKind::Blocked => colors.red,
    }
}

fn legend_item(kind: TargetKind, colors: &Colors) -> impl IntoElement {
    let color = target_color(kind, colors);
    h_flex()
        .gap(u(5.0))
        .text_size(u(11.5))
        .text_color(colors.text_dim)
        .child(div().w(u(14.0)).h(u(2.0)).bg(color))
        .child(kind.label())
}

/// Paints the edges from the current version to every target row.
fn edges(targets: Vec<(TargetKind, bool)>, colors: Colors) -> impl IntoElement {
    canvas(
        |_, _, _| (),
        move |bounds, _, window, _| {
            let scale = f32::from(window.rem_size()) / 16.0;
            let at = |x: f32, y: f32| {
                point(
                    bounds.origin.x + px(x * scale),
                    bounds.origin.y + px(y * scale),
                )
            };
            let total = targets.len() as f32 * ROW;
            let from = at(CURRENT_X, total / 2.0);
            for (ix, (kind, selected)) in targets.iter().enumerate() {
                let y = ix as f32 * ROW + ROW / 2.0;
                let to = at(TARGETS_X + 4.0, y);
                let mid = (CURRENT_X + TARGETS_X) / 2.0;
                let width = if *selected { 2.0 } else { 1.25 };
                let mut path = PathBuilder::stroke(px(width * scale));
                if matches!(kind, TargetKind::Conditional | TargetKind::Blocked) {
                    path = path.dash_array(&[px(4.0 * scale), px(3.0 * scale)]);
                }
                path.move_to(from);
                path.cubic_bezier_to(to, at(mid, total / 2.0), at(mid, y));
                let color = if *selected || *kind != TargetKind::Available {
                    target_color(*kind, &colors)
                } else {
                    colors.text_faint
                };
                if let Ok(path) = path.build() {
                    window.paint_path(path, color);
                }
            }
        },
    )
    .absolute()
    .top_0()
    .left_0()
    .size_full()
}

/// The update path: current → the offered versions, and the selected one's details.
pub fn path_card(
    view: &UpdatesView,
    status: &Status,
    target: Option<&str>,
    read_only: bool,
    cx: &mut Context<UpdatesView>,
) -> AnyElement {
    let colors = cx.colors().clone();
    let can_write = !read_only && (status.writes.control_plane || status.writes.pools);
    let selected = target.and_then(|t| status.target(t)).cloned();
    let mut header_end: Vec<AnyElement> = vec![
        legend_item(TargetKind::Recommended, &colors).into_any_element(),
        legend_item(TargetKind::Available, &colors).into_any_element(),
        legend_item(TargetKind::Conditional, &colors).into_any_element(),
        legend_item(TargetKind::Blocked, &colors).into_any_element(),
    ];
    if let Some(selected) = selected.as_ref().filter(|t| can_write && t.startable()) {
        let version = selected.version.clone();
        let scope = if status.writes.control_plane {
            Scope::ControlPlane
        } else {
            Scope::AllPools
        };
        header_end.push(
            Button::new("updates-start")
                .primary()
                .icon(IconName::ArrowUp)
                .label(format!("Update to {version}…"))
                .on_click(cx.listener(move |view, _, window, cx| {
                    view.confirm(scope.clone(), version.clone(), window, cx)
                }))
                .into_any_element(),
        );
    }
    let mut card = card(&colors).flex_1().min_w_0().child(card_header(
        "Update path",
        None,
        if status.targets.is_empty() {
            Vec::new()
        } else {
            header_end
        },
        &colors,
    ));
    if status.targets.is_empty() {
        return card
            .child(empty_path(view, status, target, &colors, cx))
            .into_any_element();
    }
    let rows: Vec<AnyElement> = status
        .targets
        .iter()
        .enumerate()
        .map(|(ix, t)| target_row(ix, t, Some(t.version.as_str()) == target, &colors, cx))
        .collect();
    let edges_input: Vec<(TargetKind, bool)> = status
        .targets
        .iter()
        .map(|t| (t.kind, Some(t.version.as_str()) == target))
        .collect();
    let graph = div()
        .relative()
        .mx(u(16.0))
        .h(u(status.targets.len() as f32 * ROW))
        .child(edges(edges_input, colors.clone()))
        .child(
            v_flex()
                .absolute()
                .left_0()
                .w(u(CURRENT_X - 12.0))
                .top(u(status.targets.len() as f32 * ROW / 2.0 - 16.0))
                .items_end()
                .child(
                    mono(status.current.version.clone())
                        .font_weight(FontWeight::MEDIUM)
                        .whitespace_nowrap(),
                )
                .child(
                    div()
                        .text_size(u(11.0))
                        .text_color(colors.green)
                        .child("current"),
                ),
        )
        .child(
            div()
                .absolute()
                .left(u(CURRENT_X - 6.0))
                .top(u(status.targets.len() as f32 * ROW / 2.0 - 6.0))
                .size(u(12.0))
                .rounded_full()
                .bg(colors.green),
        )
        .child(
            v_flex()
                .absolute()
                .left(u(TARGETS_X - 6.0))
                .right_0()
                .top_0()
                .children(rows),
        );
    card = card.child(graph);
    if let Some(selected) = selected {
        card = card.child(target_details(&selected, &colors, cx));
    }
    card.pb(u(12.0)).into_any_element()
}

/// No targets: why, and (read-only providers) the version the checks run against.
fn empty_path(
    view: &UpdatesView,
    status: &Status,
    target: Option<&str>,
    colors: &Colors,
    cx: &mut Context<UpdatesView>,
) -> AnyElement {
    let text = match &status.writes.reason {
        Some(reason) => reason.clone(),
        None if status
            .current
            .channel
            .as_deref()
            .is_some_and(|c| !c.is_empty()) =>
        {
            "No updates are offered in this channel right now.".into()
        }
        None if status.writes.pools => "Pick a pool below and the version to roll it to.".into(),
        None => "No updates are offered.".into(),
    };
    let mut col = v_flex().px(u(16.0)).pb(u(14.0)).gap(u(10.0)).child(
        div()
            .text_size(u(12.5))
            .text_color(colors.text_muted)
            .child(text),
    );
    if status.targets.is_empty() && !status.writes.pools {
        // Pre-flight checks still run against a version the user picks.
        let base = status
            .current
            .kubernetes
            .clone()
            .unwrap_or_else(|| status.current.version.clone());
        let options: Vec<String> = crate::version::Version::parse(&base)
            .map(|v| {
                (1..=2)
                    .map(|d| format!("{}.{}", v.major, v.minor + d))
                    .collect()
            })
            .unwrap_or_default();
        let weak = cx.entity().downgrade();
        let label = target.map(str::to_string).unwrap_or_else(|| "…".into());
        let _ = view;
        col = col.child(
            h_flex()
                .gap(u(8.0))
                .text_size(u(12.5))
                .child(div().text_color(colors.text_dim).child("Check against"))
                .child(
                    MenuButton::new("updates-check-target")
                        .outline()
                        .compact()
                        .child(
                            h_flex()
                                .gap(u(6.0))
                                .font_family(fonts::MONO)
                                .text_size(u(12.0))
                                .child(format!("v{label}"))
                                .child(Icon::new(IconName::ChevronDown).size(11.0)),
                        )
                        .dropdown_menu(move |mut menu, _, _| {
                            for option in &options {
                                let weak = weak.clone();
                                let value = option.clone();
                                menu =
                                    menu.item(PopupMenuItem::new(format!("v{option}")).on_click(
                                        move |_, _, cx| {
                                            let value = value.clone();
                                            weak.update(cx, |view, cx| {
                                                view.check_target = Some(value);
                                                view.rerun_preflight(cx);
                                                cx.notify();
                                            })
                                            .ok();
                                        },
                                    ));
                            }
                            menu
                        }),
                ),
        );
    }
    col.into_any_element()
}

fn target_row(
    ix: usize,
    target: &Target,
    selected: bool,
    colors: &Colors,
    cx: &mut Context<UpdatesView>,
) -> AnyElement {
    let color = target_color(target.kind, colors);
    let version = target.version.clone();
    let mut detail = Vec::new();
    if target.minor {
        detail.push("minor update".to_string());
    }
    match target.risks.len() {
        0 => {}
        1 => detail.push("1 risk".into()),
        n => detail.push(format!("{n} risks")),
    }
    if let Some(reason) = target.blocked.first() {
        detail.push(reason.clone());
    }
    let selection = colors.selection;
    h_flex()
        .id(("target", ix))
        .h(u(ROW))
        .gap(u(8.0))
        .px(u(6.0))
        .rounded(u(5.0))
        .cursor_pointer()
        .when(selected, |this| this.bg(selection))
        .hover(|s| s.bg(colors.hover))
        .on_click(cx.listener(move |view, _, _, cx| view.select(version.clone(), cx)))
        .child(
            div()
                .flex_none()
                .size(u(11.0))
                .rounded_full()
                .border_2()
                .border_color(color)
                .when(target.kind == TargetKind::Recommended, |this| {
                    this.bg(color)
                }),
        )
        .child(
            mono(target.version.clone())
                .flex_none()
                .text_size(u(12.5))
                .font_weight(if selected {
                    FontWeight::SEMIBOLD
                } else {
                    FontWeight::NORMAL
                }),
        )
        .child(
            div()
                .flex_none()
                .px(u(6.0))
                .rounded(u(4.0))
                .text_size(u(11.0))
                .text_color(color)
                .bg(color.opacity(0.12))
                .child(target.kind.label()),
        )
        .child(
            div()
                .flex_1()
                .min_w_0()
                .truncate()
                .text_size(u(11.5))
                .text_color(colors.text_dim)
                .child(detail.join(" · ")),
        )
        .into_any_element()
}

/// The selected target: notes, channels, image, risks, why it's blocked.
fn target_details(target: &Target, colors: &Colors, cx: &mut Context<UpdatesView>) -> AnyElement {
    let color = target_color(target.kind, colors);
    let mut col = v_flex()
        .mx(u(16.0))
        .mt(u(12.0))
        .pt(u(12.0))
        .gap(u(6.0))
        .border_t_1()
        .border_color(colors.border_variant)
        .child(
            h_flex()
                .gap(u(8.0))
                .child(
                    mono(target.version.clone())
                        .font_weight(FontWeight::SEMIBOLD)
                        .text_size(u(13.0)),
                )
                .child(div().text_size(u(12.0)).text_color(color).child(format!(
                    "{}{}",
                    target.kind.label(),
                    if target.minor { " · minor update" } else { "" }
                ))),
        );
    if let Some(url) = &target.url {
        let url = url.clone();
        col = col.child(kv(
            "Release notes",
            link("target-url", url.clone(), colors)
                .text_size(u(12.0))
                .on_click(move |_, window, cx| widgets::open_url(&url, window, cx)),
            104.0,
            colors,
        ));
    }
    if !target.channels.is_empty() {
        col = col.child(kv(
            "Channels",
            mono(target.channels.join(" · ")),
            104.0,
            colors,
        ));
    }
    if let Some(image) = &target.image {
        let short = match image.split_once("@sha256:") {
            Some((repo, digest)) => {
                let repo = repo.rsplit('/').next().unwrap_or(repo);
                format!("{repo}@sha256:{}…", &digest[..digest.len().min(10)])
            }
            None => image.clone(),
        };
        col = col.child(kv("Image", mono(short), 104.0, colors));
    }
    for reason in &target.blocked {
        col = col.child(
            h_flex()
                .items_start()
                .gap(u(6.0))
                .text_size(u(12.0))
                .child(Icon::new(IconName::CircleX).size(12.0).color(colors.red))
                .child(div().flex_1().min_w_0().child(reason.clone())),
        );
    }
    for (ix, risk) in target.risks.iter().enumerate() {
        let url = risk.url.clone();
        col = col.child(
            v_flex()
                .gap(u(3.0))
                .p(u(10.0))
                .rounded(u(6.0))
                .border_1()
                .border_color(colors.yellow.opacity(0.5))
                .bg(colors.yellow.opacity(0.05))
                .text_size(u(12.0))
                .child(
                    h_flex()
                        .gap(u(6.0))
                        .child(
                            Icon::new(IconName::TriangleAlert)
                                .size(12.0)
                                .color(colors.yellow),
                        )
                        .child(mono(risk.name.clone()).text_color(colors.yellow)),
                )
                .child(
                    div()
                        .text_color(colors.text_muted)
                        .child(risk.message.clone()),
                )
                .when_some(url, |this, url| {
                    this.child(
                        link(("risk-link", ix), "Learn more", colors)
                            .on_click(move |_, window, cx| widgets::open_url(&url, window, cx)),
                    )
                }),
        );
    }
    if target.kind == TargetKind::Conditional {
        col = col.child(
            div()
                .text_size(u(11.5))
                .text_color(colors.text_dim)
                .child("Starting it asks you to accept each risk, like oc adm upgrade --allow-not-recommended."),
        );
    }
    let _ = cx;
    col.into_any_element()
}

/// Runs a check's fix.
pub fn run_fix(cluster: &ClusterId, fix: &Fix, window: &mut Window, cx: &mut App) {
    match fix {
        Fix::Open { link, .. } => {
            let target = ResourceRef::object(
                cluster.clone(),
                link.gvr.clone(),
                link.namespace.clone(),
                link.name.clone(),
            );
            window.dispatch_action(
                Box::new(OpenView(ViewRequest::for_resource(
                    ViewKind::Details,
                    target,
                ))),
                cx,
            );
        }
        Fix::OpenRelease {
            namespace,
            name,
            object,
            secret,
        } => {
            let row = kubyl_operators::helm::service::Helm::global(cx)
                .and_then(|h| h.read(cx).snapshot(cluster, cx))
                .and_then(|s| {
                    s.releases
                        .iter()
                        .find(|r| &r.namespace == namespace && &r.name == name)
                        .cloned()
                });
            match row {
                Some(row) => kubyl_operators::release::open(
                    cluster,
                    &row,
                    Some(kubyl_operators::release::ReleaseTab::Manifest),
                    window,
                    cx,
                ),
                None => {
                    let resource = if *secret { "secrets" } else { "configmaps" };
                    let target = ResourceRef::object(
                        cluster.clone(),
                        kubyl_core::Gvr::new("", "v1", resource),
                        Some(namespace.clone()),
                        object.clone(),
                    );
                    window.dispatch_action(
                        Box::new(OpenView(ViewRequest::for_resource(
                            ViewKind::Details,
                            target,
                        ))),
                        cx,
                    );
                }
            }
        }
        Fix::Copy { text, .. } => {
            cx.write_to_clipboard(ClipboardItem::new_string(text.clone()));
            NotificationCenter::push(cx, Notification::info("Copied the command."));
        }
        Fix::Url { url, .. } => widgets::open_url(url, window, cx),
        Fix::Operators => kubyl_operators::view::open(
            cluster,
            kubyl_operators::view::Pending::default(),
            window,
            cx,
        ),
    }
}

fn fix_button(
    id: impl Into<gpui::ElementId>,
    cluster: &ClusterId,
    fix: &Fix,
    colors: &Colors,
) -> AnyElement {
    let cluster = cluster.clone();
    let fix = fix.clone();
    row_button(id, fix.label().to_string(), colors)
        .on_click(move |_, window, cx| run_fix(&cluster, &fix, window, cx))
        .into_any_element()
}

fn check_row(
    view: &UpdatesView,
    ix: usize,
    check: &Check,
    colors: &Colors,
    cx: &mut Context<UpdatesView>,
) -> AnyElement {
    let (icon, color) = status_icon(check.status, colors);
    let expandable = !check.details.is_empty();
    let expanded = view.expanded.contains(check.id);
    let id = check.id;
    let mut row = v_flex()
        .py(u(10.0))
        .border_t_1()
        .border_color(colors.row_border)
        .child(
            h_flex()
                .id(("check", ix))
                .items_start()
                .gap(u(10.0))
                .when(expandable, |this| {
                    this.cursor_pointer()
                        .on_click(cx.listener(move |view, _, _, cx| {
                            if !view.expanded.remove(id) {
                                view.expanded.insert(id);
                            }
                            cx.notify();
                        }))
                })
                .child(
                    div()
                        .mt(u(2.0))
                        .child(Icon::new(icon).size(14.0).color(color)),
                )
                .child(
                    v_flex()
                        .flex_1()
                        .min_w_0()
                        .child(
                            h_flex()
                                .gap(u(6.0))
                                .text_size(u(12.5))
                                .child(check.title.clone())
                                .when(expandable, |this| {
                                    this.child(
                                        Icon::new(if expanded {
                                            IconName::ChevronDown
                                        } else {
                                            IconName::ChevronRight
                                        })
                                        .size(11.0)
                                        .color(colors.text_dim),
                                    )
                                }),
                        )
                        .child(
                            div()
                                .text_size(u(11.5))
                                .text_color(colors.text_dim)
                                .child(check.summary.clone()),
                        ),
                )
                .when_some(check.fix.as_ref(), |this, fix| {
                    this.child(fix_button(("check-fix", ix), &view.cluster, fix, colors))
                }),
        );
    if expanded {
        let mut list = v_flex().pl(u(24.0)).pt(u(6.0)).gap(u(4.0));
        for (dx, detail) in check.details.iter().enumerate() {
            let (icon, color) = status_icon(detail.status, colors);
            list = list.child(
                h_flex()
                    .items_start()
                    .gap(u(8.0))
                    .text_size(u(12.0))
                    .child(
                        div()
                            .mt(u(2.0))
                            .child(Icon::new(icon).size(11.0).color(color)),
                    )
                    .child(
                        v_flex()
                            .flex_1()
                            .min_w_0()
                            .child(div().child(detail.text.clone()))
                            .when_some(detail.sub.clone(), |this, sub| {
                                this.child(
                                    div()
                                        .font_family(fonts::MONO)
                                        .text_size(u(11.0))
                                        .text_color(colors.text_dim)
                                        .child(sub),
                                )
                            }),
                    )
                    .when_some(detail.fix.as_ref(), |this, fix| {
                        this.child(fix_button(
                            gpui::ElementId::NamedInteger(
                                format!("detail-fix-{ix}").into(),
                                dx as u64,
                            ),
                            &view.cluster,
                            fix,
                            colors,
                        ))
                    }),
            );
        }
        row = row.child(list);
    }
    row.into_any_element()
}

/// Pre-flight results for `target`: problems first, the passed ones folded.
pub fn preflight_card(
    view: &UpdatesView,
    status: &Status,
    target: &str,
    cx: &mut Context<UpdatesView>,
) -> AnyElement {
    let colors = cx.colors().clone();
    let run =
        Updates::global(cx).and_then(|u| u.read(cx).preflight(&view.cluster, target).cloned());
    let checks = view.checks(target, cx);
    let label = match status.provider.contains("read-only") && status.targets.is_empty() {
        true => format!("Pre-flight checks against v{target}"),
        false => format!("Pre-flight checks for {target}"),
    };
    let mut end: Vec<AnyElement> = Vec::new();
    if let Some(run) = &run {
        end.push(
            div()
                .text_size(u(12.0))
                .text_color(colors.text_dim)
                .child(match run.finished {
                    Some(finished) => format!(
                        "ran {} ago",
                        kubyl_resources::format::human_duration(finished.elapsed().as_secs() as i64)
                    ),
                    None => "running…".into(),
                })
                .into_any_element(),
        );
    }
    end.push(
        Button::new("updates-rerun")
            .ghost()
            .icon(IconName::RefreshCw)
            .label("Re-run")
            .on_click(cx.listener(|view, _, _, cx| view.rerun_preflight(cx)))
            .into_any_element(),
    );
    let summary = checks
        .as_ref()
        .filter(|_| run.as_ref().is_some_and(|r| !r.running()))
        .map(|c| Counts::of(c).summary());
    let mut body = v_flex().px(u(16.0)).pb(u(6.0));
    match (&checks, run.as_ref().map(|r| r.running())) {
        (Some(checks), Some(false)) => {
            let (problems, passed): (Vec<&Check>, Vec<&Check>) = checks.iter().partition(|c| {
                matches!(
                    c.status,
                    CheckStatus::Fail | CheckStatus::Warn | CheckStatus::Unknown
                )
            });
            for (ix, check) in problems.iter().enumerate() {
                body = body.child(check_row(view, ix, check, &colors, cx));
            }
            if view.show_passed {
                for (ix, check) in passed.iter().enumerate() {
                    body = body.child(check_row(view, 100 + ix, check, &colors, cx));
                }
            } else if !passed.is_empty() {
                let titles: Vec<String> = passed
                    .iter()
                    .filter(|c| c.status == CheckStatus::Pass)
                    .map(|c| c.title.clone())
                    .collect();
                body = body.child(
                    h_flex()
                        .gap(u(10.0))
                        .py(u(10.0))
                        .border_t_1()
                        .border_color(colors.row_border)
                        .child(
                            Icon::new(IconName::CircleCheck)
                                .size(14.0)
                                .color(colors.green),
                        )
                        .child(
                            div()
                                .flex_1()
                                .min_w_0()
                                .text_size(u(12.0))
                                .text_color(colors.text_muted)
                                .child(format!("{} passed: {}", titles.len(), titles.join(", "))),
                        )
                        .child(
                            link("show-passed", "Show all", &colors)
                                .text_size(u(12.0))
                                .on_click(cx.listener(|view, _, _, cx| {
                                    view.show_passed = true;
                                    cx.notify();
                                })),
                        ),
                );
            }
        }
        _ => {
            body = body.child(
                h_flex()
                    .gap(u(8.0))
                    .py(u(10.0))
                    .text_size(u(12.5))
                    .text_color(colors.text_dim)
                    .child(Icon::new(IconName::RefreshCw).size(13.0).color(colors.text_dim))
                    .child("Checking PodDisruptionBudgets, deprecated and removed APIs, Helm releases, node headroom, version skew and operators…"),
            );
        }
    }
    card(&colors)
        .child(card_header(label, summary, end, &colors))
        .child(body)
        .into_any_element()
}

fn tri(value: Option<bool>, good: bool, colors: &Colors) -> impl IntoElement {
    let (text, color) = match value {
        Some(v) => (
            if v { "True" } else { "False" },
            if v == good {
                colors.green
            } else {
                colors.yellow
            },
        ),
        None => ("–", colors.text_dim),
    };
    div()
        .font_family(fonts::MONO)
        .text_size(u(12.0))
        .text_color(color)
        .child(text)
}

/// ClusterOperators: working ones first; the rest folds after 10.
pub fn components_card(
    view: &UpdatesView,
    status: &Status,
    cx: &mut Context<UpdatesView>,
) -> AnyElement {
    let colors = cx.colors().clone();
    let total = status.components.len();
    let target = status
        .progress
        .as_ref()
        .map(|p| p.target.clone())
        .unwrap_or_else(|| status.current.version.clone());
    let updated = status.components.iter().filter(|c| c.updated).count();
    let title = if status.progress.is_some() {
        format!("Cluster operators · {updated} of {total} updated")
    } else {
        format!("Cluster operators · {updated} of {total} at {target}")
    };
    let limit = if view.show_all_components {
        total
    } else {
        total.min(10)
    };
    let mut table = v_flex().child(table_header(
        &[
            ("Name", Some(200.0)),
            ("Version", Some(150.0)),
            ("Available", Some(80.0)),
            ("Progressing", Some(90.0)),
            ("Degraded", Some(80.0)),
            ("Message", None),
        ],
        &colors,
    ));
    for component in status.components.iter().take(limit) {
        table = table.child(component_row(component, &target, &colors));
    }
    let mut end = Vec::new();
    if total > 10 {
        end.push(
            link(
                "components-all",
                if view.show_all_components {
                    "Show fewer".to_string()
                } else {
                    format!("Show all {total}")
                },
                &colors,
            )
            .text_size(u(12.0))
            .on_click(cx.listener(|view, _, _, cx| {
                view.show_all_components = !view.show_all_components;
                cx.notify();
            }))
            .into_any_element(),
        );
    }
    card(&colors)
        .child(card_header(
            title,
            Some("progressing first".into()),
            end,
            &colors,
        ))
        .child(table)
        .into_any_element()
}

fn component_row(c: &Component, target: &str, colors: &Colors) -> impl IntoElement {
    let settled = c.updated && c.progressing != Some(true) && c.degraded != Some(true);
    let dim = if settled {
        colors.text_muted
    } else {
        colors.text
    };
    let version = match (&c.version, c.updated) {
        (Some(v), true) => v.clone(),
        (Some(v), false) => format!("{v} → {target}"),
        (None, _) => "–".into(),
    };
    table_row(colors)
        .text_color(dim)
        .child(cell(Some(200.0), mono(c.name.clone())))
        .child(cell(
            Some(150.0),
            mono(version).text_color(if c.updated { dim } else { colors.accent }),
        ))
        .child(cell(Some(80.0), tri(c.available, true, colors)))
        .child(cell(Some(90.0), tri(c.progressing, false, colors)))
        .child(cell(Some(80.0), tri(c.degraded, false, colors)))
        .child(cell(
            None,
            div()
                .truncate()
                .text_size(u(12.0))
                .text_color(if c.degraded == Some(true) {
                    colors.red
                } else {
                    colors.text_dim
                })
                .child(c.message.clone().unwrap_or_default()),
        ))
}

/// Control plane and pools: MachineConfigPools, node groups, Plans, MachineDeployments.
pub fn pools_card(
    view: &UpdatesView,
    status: &Status,
    target: Option<&str>,
    read_only: bool,
    _: &mut Window,
    cx: &mut Context<UpdatesView>,
) -> AnyElement {
    let colors = cx.colors().clone();
    let kind = status.pools.first().map(|p| p.kind);
    let (title, note) = match kind {
        Some(PoolKind::MachineConfigPool) => (
            "Machine config pools",
            "nodes update one at a time per pool (maxUnavailable)",
        ),
        Some(PoolKind::Plan) => ("Upgrade plans", "system-upgrade-controller"),
        Some(PoolKind::MachineDeployment) | Some(PoolKind::ControlPlane)
            if status.provider.starts_with("Cluster API") =>
        {
            ("Workload clusters", "control planes and MachineDeployments")
        }
        Some(PoolKind::Nodes) | Some(PoolKind::ControlPlane)
            if status.provider.contains("read-only") =>
        {
            ("Nodes", "kubelet versions by role")
        }
        _ => (
            "Control plane & node pools",
            "control plane first, then the pools",
        ),
    };
    let capi = status.provider.starts_with("Cluster API");
    let can_write = !read_only && status.writes.pools;
    let mut table = v_flex().child(table_header(
        &[
            ("Pool", None),
            ("Version", Some(170.0)),
            ("Nodes updated", Some(150.0)),
            ("Ready", Some(50.0)),
            ("State", Some(230.0)),
            ("", Some(120.0)),
        ],
        &colors,
    ));
    for (ix, pool) in status.pools.iter().enumerate() {
        let state: (String, Hsla) = match pool.state {
            PoolState::Updating => (
                pool.draining.clone().unwrap_or_else(|| "updating".into()),
                colors.accent,
            ),
            PoolState::Degraded => ("degraded".into(), colors.red),
            PoolState::Paused => ("paused".into(), colors.yellow),
            PoolState::Queued => ("queued".into(), colors.text_dim),
            PoolState::Idle => (
                pool.message.clone().unwrap_or_else(|| "up to date".into()),
                colors.text_dim,
            ),
        };
        let progress = match (pool.updated, pool.nodes) {
            (Some(updated), Some(nodes)) if nodes > 0 => h_flex()
                .gap(u(8.0))
                .child(mono(format!("{updated} / {nodes}")))
                .child(bar(
                    updated as f32 * 100.0 / nodes as f32,
                    70.0,
                    if updated == nodes {
                        colors.green
                    } else {
                        colors.accent
                    },
                    &colors,
                ))
                .into_any_element(),
            (_, Some(nodes)) => mono(match nodes {
                1 => "1 node".to_string(),
                n => format!("{n} nodes"),
            })
            .into_any_element(),
            _ => div().into_any_element(),
        };
        let action: AnyElement = if can_write && pool.updatable {
            let id = pool.id.clone();
            let pool_version = pool.version.clone().unwrap_or_default();
            // A cloud node pool follows its control plane; Plans move to the selected version.
            let default_target = match pool.kind {
                PoolKind::NodePool => Some(status.current.version.clone()),
                _ => target.map(str::to_string),
            };
            let name = pool.name.clone();
            row_button(("pool-update", ix), "Update…", &colors)
                .on_click(cx.listener(move |view, _, window, cx| {
                    if capi {
                        // Cluster API has no update graph: ask for the version.
                        let id = id.clone();
                        let weak = cx.entity().downgrade();
                        kubyl_explorer::dialogs::prompt_text(
                            format!("Update {name}").into(),
                            "Kubernetes version (e.g. v1.31.4)",
                            pool_version.clone(),
                            move |version, window, cx| {
                                let id = id.clone();
                                weak.update(cx, |view, cx| {
                                    view.confirm(
                                        Scope::Pool(id),
                                        version.trim().to_string(),
                                        window,
                                        cx,
                                    )
                                })
                                .ok();
                            },
                            window,
                            cx,
                        );
                    } else if let Some(target) = default_target.clone() {
                        view.confirm(Scope::Pool(id.clone()), target, window, cx);
                    }
                }))
                .into_any_element()
        } else {
            div().into_any_element()
        };
        let object = pool.object.clone();
        let cluster = view.cluster.clone();
        table = table.child(
            table_row(&colors)
                .child(cell(
                    None,
                    v_flex()
                        .child(
                            div()
                                .id(("pool-name", ix))
                                .font_weight(FontWeight::MEDIUM)
                                .truncate()
                                .when(object.is_some(), |this| {
                                    this.cursor_pointer().hover(|s| s.underline())
                                })
                                .child(pool.name.clone())
                                .on_click(move |_, window, cx| {
                                    if let Some(link) = &object {
                                        run_fix(
                                            &cluster,
                                            &Fix::Open {
                                                label: String::new(),
                                                link: link.clone(),
                                            },
                                            window,
                                            cx,
                                        );
                                    }
                                }),
                        )
                        .when_some(pool.surge.clone(), |this, surge| {
                            this.child(
                                div()
                                    .text_size(u(11.0))
                                    .text_color(colors.text_dim)
                                    .child(surge),
                            )
                        }),
                ))
                .child(cell(
                    Some(170.0),
                    mono(pool.version.clone().unwrap_or_default()).truncate(),
                ))
                .child(cell(Some(150.0), progress))
                .child(cell(
                    Some(50.0),
                    mono(pool.ready.map(|r| r.to_string()).unwrap_or_default()),
                ))
                .child(cell(
                    Some(230.0),
                    div()
                        .truncate()
                        .text_size(u(12.0))
                        .text_color(state.1)
                        .child(state.0),
                ))
                .child(cell(Some(120.0), h_flex().justify_end().child(action))),
        );
    }
    let mut end = Vec::new();
    if kind == Some(PoolKind::Plan) && can_write {
        let kind = if status.provider.starts_with("RKE2") {
            ProviderKind::Rke2
        } else {
            ProviderKind::K3s
        };
        let version = target
            .map(str::to_string)
            .unwrap_or_else(|| status.current.version.clone());
        let cluster = view.cluster.clone();
        end.push(
            row_button("new-plans", "New plans…", &colors)
                .on_click(move |_, window, cx| {
                    let list = ResourceRef::list(
                        cluster.clone(),
                        kubyl_core::Gvr::new("upgrade.cattle.io", "v1", "plans"),
                        Some("system-upgrade".into()),
                    );
                    kubyl_yaml::open_draft(
                        list,
                        crate::suc::plan_templates(kind, &version),
                        Some("system-upgrade-controller Plans: check the node selectors, then apply. The controller starts updating matching nodes right away.".into()),
                        window,
                        cx,
                    );
                })
                .into_any_element(),
        );
    }
    card(&colors)
        .child(card_header(title, Some(note.into()), end, &colors))
        .child(table)
        .into_any_element()
}

/// Managed add-ons and their compatibility with the target.
pub fn addons_card(
    view: &UpdatesView,
    status: &Status,
    read_only: bool,
    cx: &mut Context<UpdatesView>,
) -> AnyElement {
    let colors = cx.colors().clone();
    let can_write = !read_only && status.writes.addons;
    let mut table = v_flex().child(table_header(
        &[
            ("Add-on", None),
            ("Version", Some(170.0)),
            ("Compatible with target", Some(200.0)),
            ("", Some(140.0)),
        ],
        &colors,
    ));
    for (ix, addon) in status.addons.iter().enumerate() {
        let compatible: AnyElement = match (addon.compatible, &addon.recommended) {
            (Some(true), _) => {
                widgets::icon_text(IconName::CircleCheck, colors.green, "yes").into_any_element()
            }
            (Some(false), Some(rec)) => widgets::icon_text(
                IconName::TriangleAlert,
                colors.yellow,
                format!("needs {rec}"),
            )
            .into_any_element(),
            (Some(false), None) => {
                widgets::icon_text(IconName::TriangleAlert, colors.yellow, "no").into_any_element()
            }
            (None, _) => div()
                .text_color(colors.text_dim)
                .child("unknown")
                .into_any_element(),
        };
        let action: AnyElement = match (&addon.recommended, can_write && addon.updatable) {
            (Some(recommended), true) if addon.compatible == Some(false) => {
                let name = addon.name.clone();
                let recommended = recommended.clone();
                row_button(("addon-update", ix), "Update add-on…", &colors)
                    .on_click(cx.listener(move |view, _, window, cx| {
                        view.confirm(Scope::AddOn(name.clone()), recommended.clone(), window, cx)
                    }))
                    .into_any_element()
            }
            _ => div().into_any_element(),
        };
        table = table.child(
            table_row(&colors)
                .child(cell(None, mono(addon.name.clone())))
                .child(cell(Some(170.0), mono(addon.version.clone())))
                .child(cell(Some(200.0), compatible))
                .child(cell(Some(140.0), h_flex().justify_end().child(action))),
        );
    }
    let _ = view;
    card(&colors)
        .child(card_header("Add-ons", None, Vec::new(), &colors))
        .child(table)
        .into_any_element()
}

/// Past updates.
pub fn history_card(status: &Status, colors: &Colors) -> AnyElement {
    let mut table = v_flex().child(table_header(
        &[
            ("Version", Some(140.0)),
            ("State", Some(110.0)),
            ("Started", Some(150.0)),
            ("Took", Some(110.0)),
            ("", None),
        ],
        colors,
    ));
    for entry in status.history.iter().take(10) {
        let state_color = if entry.completed() {
            colors.green
        } else {
            colors.accent
        };
        table = table.child(
            table_row(colors)
                .child(cell(Some(140.0), mono(entry.version.clone())))
                .child(cell(
                    Some(110.0),
                    div().text_color(state_color).child(entry.state.clone()),
                ))
                .child(cell(
                    Some(150.0),
                    div()
                        .text_color(colors.text_muted)
                        .child(entry.started.map(date).unwrap_or_default()),
                ))
                .child(cell(
                    Some(110.0),
                    div().text_color(colors.text_muted).child(
                        match (entry.started, entry.completed) {
                            (Some(s), Some(c)) => took(s, c),
                            _ => String::new(),
                        },
                    ),
                ))
                .child(cell(
                    None,
                    div()
                        .truncate()
                        .text_size(u(12.0))
                        .text_color(colors.text_dim)
                        .child(match (&entry.accepted_risks, entry.verified) {
                            (Some(risks), _) => format!("accepted risks: {risks}"),
                            (None, true) => "verified".to_string(),
                            (None, false) => String::new(),
                        }),
                )),
        );
    }
    card(colors)
        .child(card_header("Update history", None, Vec::new(), colors))
        .child(table)
        .into_any_element()
}

/// How to update a cluster Kubyl can't update.
pub fn docs_card(status: &Status, colors: &Colors) -> AnyElement {
    let mut col = v_flex().px(u(16.0)).pb(u(12.0)).gap(u(6.0));
    for (ix, (label, url)) in status.docs.iter().enumerate() {
        let url = url.clone();
        col = col.child(
            h_flex()
                .gap(u(6.0))
                .text_size(u(12.5))
                .child(
                    Icon::new(IconName::ExternalLink)
                        .size(12.0)
                        .color(colors.accent),
                )
                .child(
                    link(("doc", ix), SharedString::from(label.clone()), colors)
                        .on_click(move |_, window, cx| widgets::open_url(&url, window, cx)),
                ),
        );
    }
    card(colors)
        .child(card_header(
            "How to update this cluster",
            None,
            Vec::new(),
            colors,
        ))
        .child(col)
        .into_any_element()
}
