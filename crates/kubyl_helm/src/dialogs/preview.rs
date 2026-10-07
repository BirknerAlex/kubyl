//! The preview pane of the install, upgrade and rollback dialogs (decision 5): the objects a dry
//! run rendered (install) or the per-object diff against the current release (upgrade,
//! rollback), the values diff (masked until revealed), the hooks that run and the CRDs Helm
//! installs once. Secret data is masked before anything is shown.

use std::sync::Arc;

use gpui::{AnyElement, Context, IntoElement, Render, SharedString, Window, div, prelude::*};
use kubyl_helm_core::present;
use kubyl_helm_core::preview::{Change, Counts, ObjectChange, Preview};
use kubyl_ui::{ActiveColors, Button, Colors, Icon, IconName, fonts, h_flex, u, v_flex};

use crate::widgets;

/// What the preview is for.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum PreviewKind {
    Install,
    Upgrade,
    Rollback,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Tab {
    Objects,
    Values,
}

pub struct PreviewPane {
    kind: PreviewKind,
    preview: Arc<Preview>,
    selected: usize,
    tab: Tab,
    reveal: bool,
    side_by_side: bool,
    /// `--no-hooks` is on: no hook runs, so none is listed.
    hooks_skipped: bool,
}

impl PreviewPane {
    pub fn new(kind: PreviewKind, preview: Arc<Preview>) -> Self {
        Self {
            kind,
            preview,
            selected: 0,
            tab: Tab::Objects,
            reveal: false,
            side_by_side: false,
            hooks_skipped: false,
        }
    }

    /// Whether `--no-hooks` is on (the hooks aren't listed then).
    pub fn set_hooks_skipped(&mut self, skipped: bool, cx: &mut Context<Self>) {
        self.hooks_skipped = skipped;
        cx.notify();
    }

    /// The hooks that will run.
    pub fn hooks(&self) -> &[kubyl_helm_core::preview::HookInfo] {
        if self.hooks_skipped {
            &[]
        } else {
            &self.preview.hooks
        }
    }

    pub fn preview(&self) -> &Preview {
        &self.preview
    }

    pub fn counts(&self) -> Counts {
        Counts::of(&self.preview.changes)
    }

    /// Whether the change list has anything for the run button (an upgrade may change nothing).
    pub fn changes_anything(&self) -> bool {
        let counts = self.counts();
        counts.changed + counts.added + counts.removed > 0
            || self.preview.values.as_ref().is_some_and(|v| !v.is_empty())
    }

    fn banner(&self, colors: &Colors) -> AnyElement {
        let counts = self.counts();
        let (icon, color, bg, text) = if self.preview.client_side {
            (
                IconName::TriangleAlert,
                colors.yellow,
                colors.yellow.opacity(0.1),
                format!(
                    "Rendered client-side: the server-side dry run isn't allowed (it needs get access for lookups). The preview is less exact. {}.",
                    counts.label()
                ),
            )
        } else {
            let what = match self.kind {
                PreviewKind::Install => format!(
                    "Server-side dry run passed: {}, {}{}. Nothing was applied.",
                    plural(self.preview.changes.len(), "object"),
                    plural(self.hooks().len(), "hook"),
                    if self.preview.crds.is_empty() {
                        String::new()
                    } else {
                        format!(", {}", plural(self.preview.crds.len(), "CRD"))
                    }
                ),
                PreviewKind::Upgrade => format!(
                    "Server-side dry run passed: {}. Nothing was applied.",
                    counts.label()
                ),
                PreviewKind::Rollback => format!(
                    "From the stored revisions: {}. Nothing was applied.",
                    counts.label()
                ),
            };
            (
                IconName::CircleCheck,
                colors.green,
                colors.green.opacity(0.08),
                what,
            )
        };
        h_flex()
            .flex_none()
            .gap(u(8.0))
            .px(u(16.0))
            .py(u(8.0))
            .bg(bg)
            .border_b_1()
            .border_color(colors.border_variant)
            .text_size(u(12.5))
            .child(Icon::new(icon).size(13.0).color(color))
            .child(div().flex_1().min_w_0().child(text))
            .into_any_element()
    }

    /// The upgrade's warnings: CRDs Helm won't apply, hooks that run.
    fn warnings(&self, colors: &Colors) -> Option<AnyElement> {
        if self.kind == PreviewKind::Install {
            return None;
        }
        let mut parts = Vec::new();
        if self.kind == PreviewKind::Upgrade && !self.preview.crds.is_empty() {
            parts.push(format!(
                "The chart's CRDs ({}) aren't applied by an upgrade: Helm only installs CRDs.",
                self.preview.crds.join(", ")
            ));
        }
        if !self.hooks().is_empty() {
            parts.push(format!(
                "Hooks run: {}.",
                self.hooks()
                    .iter()
                    .map(|h| format!("{} {} ({})", h.kind, h.name, h.events.join(", ")))
                    .collect::<Vec<_>>()
                    .join("; ")
            ));
        }
        let api_changes: Vec<String> = self
            .preview
            .changes
            .iter()
            .filter_map(|c| {
                c.api_version
                    .as_ref()
                    .map(|(a, b)| format!("{} {}: {a} → {b}", c.key.kind, c.key.name))
            })
            .collect();
        if !api_changes.is_empty() {
            parts.push(format!("API versions change: {}.", api_changes.join("; ")));
        }
        if parts.is_empty() {
            return None;
        }
        Some(
            h_flex()
                .flex_none()
                .items_start()
                .gap(u(8.0))
                .px(u(16.0))
                .py(u(7.0))
                .bg(colors.yellow.opacity(0.08))
                .border_b_1()
                .border_color(colors.border_variant)
                .text_size(u(12.0))
                .child(
                    Icon::new(IconName::TriangleAlert)
                        .size(13.0)
                        .color(colors.yellow),
                )
                .child(div().flex_1().min_w_0().child(parts.join(" ")))
                .into_any_element(),
        )
    }

    fn object_row(
        &self,
        index: usize,
        change: &ObjectChange,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let colors = cx.colors().clone();
        let (mark, color, word) = match change.change {
            Change::Added if self.kind == PreviewKind::Install => ("", colors.text_dim, ""),
            Change::Added => ("+", colors.green, "added"),
            Change::Removed => ("−", colors.red, "removed"),
            Change::Changed => ("~", colors.yellow, "changed"),
            Change::Unchanged => ("", colors.text_dim, ""),
        };
        let selected = index == self.selected;
        h_flex()
            .id(("preview-object", index))
            .gap(u(8.0))
            .px(u(8.0))
            .py(u(4.0))
            .rounded(u(5.0))
            .cursor_pointer()
            .text_size(u(12.0))
            .when(selected, |this| {
                this.bg(colors.selection)
                    .border_1()
                    .border_color(colors.accent)
            })
            .when(!selected, |this| this.hover(|s| s.bg(colors.hover)))
            .child(
                div()
                    .flex_none()
                    .w(u(10.0))
                    .font_family(fonts::MONO)
                    .text_color(color)
                    .child(mark),
            )
            .child(
                div()
                    .flex_none()
                    .w(u(96.0))
                    .truncate()
                    .text_color(colors.text_dim)
                    .child(change.key.kind.clone()),
            )
            .child(
                div()
                    .flex_1()
                    .min_w_0()
                    .truncate()
                    .font_family(fonts::MONO)
                    .text_size(u(11.5))
                    .child(change.key.name.clone()),
            )
            .child(
                div()
                    .flex_none()
                    .text_size(u(11.0))
                    .text_color(color)
                    .child(word),
            )
            .on_click(cx.listener(move |this, _, _, cx| {
                this.selected = index;
                this.tab = Tab::Objects;
                cx.notify();
            }))
            .into_any_element()
    }

    fn object_list(&self, cx: &mut Context<Self>) -> AnyElement {
        let colors = cx.colors().clone();
        let mut list = v_flex().gap(u(1.0));
        if self.kind == PreviewKind::Install {
            // Grouped by kind, workloads first (the list is already in that order).
            let mut last_kind = String::new();
            for (i, change) in self.preview.changes.iter().enumerate() {
                if change.key.kind != last_kind {
                    let count = self
                        .preview
                        .changes
                        .iter()
                        .filter(|c| c.key.kind == change.key.kind)
                        .count();
                    list = list.child(
                        div()
                            .pt(u(6.0))
                            .px(u(8.0))
                            .text_size(u(10.5))
                            .text_color(colors.text_dim)
                            .child(format!("{} · {count}", change.key.kind.to_uppercase())),
                    );
                    last_kind = change.key.kind.clone();
                }
                list = list.child(self.object_row(i, change, cx));
            }
        } else {
            for (i, change) in self.preview.changes.iter().enumerate() {
                list = list.child(self.object_row(i, change, cx));
            }
        }
        v_flex()
            .id("preview-objects")
            .flex_none()
            .w(u(280.0))
            .h_full()
            .p(u(8.0))
            .border_r_1()
            .border_color(colors.border_variant)
            .overflow_y_scroll()
            .child(
                div()
                    .px(u(6.0))
                    .pb(u(4.0))
                    .text_size(u(11.0))
                    .text_color(colors.text_dim)
                    .child(match self.kind {
                        PreviewKind::Install => {
                            format!("OBJECTS · {}", self.preview.changes.len())
                        }
                        _ => format!("OBJECTS · {}", self.counts().label().to_uppercase()),
                    }),
            )
            .child(list)
            .into_any_element()
    }

    fn object_body(&self, cx: &mut Context<Self>) -> AnyElement {
        let colors = cx.colors().clone();
        let Some(change) = self.preview.changes.get(self.selected) else {
            return widgets::empty("The chart renders no objects.", &colors);
        };
        let body: AnyElement = match change.change {
            Change::Changed if !change.diff.is_empty() => {
                kubyl_yaml::diff_view(&change.diff, self.side_by_side, &colors)
            }
            Change::Changed if change.secret_data_changed => widgets::note(
                IconName::Lock,
                colors.yellow,
                "The Secret's data changes. Its values are masked, so the diff can't show them.",
                &colors,
            ),
            _ => v_flex()
                .pt(u(4.0))
                .children(
                    present::text_lines(&change.text)
                        .iter()
                        .enumerate()
                        .map(|(i, line)| widgets::yaml_line(i + 1, line, &colors)),
                )
                .into_any_element(),
        };
        let what = match (self.kind, &change.change) {
            (PreviewKind::Install, _) => "rendered".to_string(),
            (_, Change::Added) => "added".into(),
            (_, Change::Removed) => "removed".into(),
            (_, Change::Unchanged) => "unchanged".into(),
            (PreviewKind::Upgrade, Change::Changed) => "current → new".into(),
            (PreviewKind::Rollback, Change::Changed) => "current → target revision".into(),
        };
        let toggle = (change.change == Change::Changed && !change.diff.is_empty()).then(|| {
            widgets::toggle_chip(
                "preview-side-by-side",
                "side by side",
                self.side_by_side,
                cx.listener(|this, _, _, cx| {
                    this.side_by_side = !this.side_by_side;
                    cx.notify();
                }),
            )
        });
        let secret = change.key.kind == "Secret";
        v_flex()
            .flex_1()
            .min_w_0()
            .min_h_0()
            .child(
                h_flex()
                    .flex_none()
                    .h(u(32.0))
                    .px(u(12.0))
                    .gap(u(8.0))
                    .border_b_1()
                    .border_color(colors.border_variant)
                    .text_size(u(12.0))
                    .child(div().font_family(fonts::MONO).child(change.key.label()))
                    .child(div().text_color(colors.text_dim).child(format!("· {what}")))
                    .child(div().flex_1())
                    .when(secret, |this| {
                        this.child(
                            h_flex()
                                .gap(u(5.0))
                                .text_color(colors.text_dim)
                                .child(Icon::new(IconName::Lock).size(12.0).color(colors.text_dim))
                                .child("Secret data is masked"),
                        )
                    })
                    .children(toggle),
            )
            .child(
                div()
                    .id("preview-object-body")
                    .flex_1()
                    .min_h_0()
                    .overflow_scroll()
                    .child(body),
            )
            .into_any_element()
    }

    fn values_body(&self, cx: &mut Context<Self>) -> AnyElement {
        let colors = cx.colors().clone();
        let Some(diff) = &self.preview.values else {
            return widgets::empty("", &colors);
        };
        let body = if diff.is_empty() {
            widgets::empty("The user-supplied values don't change.", &colors)
        } else if self.reveal {
            kubyl_yaml::diff_view(diff, false, &colors)
        } else {
            kubyl_yaml::diff_view(&kubyl_helm_core::values::masked(diff), false, &colors)
        };
        v_flex()
            .flex_1()
            .min_w_0()
            .min_h_0()
            .child(
                h_flex()
                    .flex_none()
                    .h(u(32.0))
                    .px(u(12.0))
                    .gap(u(8.0))
                    .border_b_1()
                    .border_color(colors.border_variant)
                    .text_size(u(12.0))
                    .text_color(colors.text_dim)
                    .child(Icon::new(IconName::Lock).size(12.0).color(colors.text_dim))
                    .child(if self.reveal {
                        "Values shown: they can hold passwords."
                    } else {
                        "User-supplied values, masked: which keys change, not what they hold."
                    })
                    .child(div().flex_1())
                    .child(
                        Button::new("preview-reveal")
                            .ghost()
                            .icon(if self.reveal {
                                IconName::EyeOff
                            } else {
                                IconName::Eye
                            })
                            .label(if self.reveal { "Mask" } else { "Reveal" })
                            .on_click(cx.listener(|this, _, _, cx| {
                                this.reveal = !this.reveal;
                                cx.notify();
                            })),
                    ),
            )
            .child(
                div()
                    .id("preview-values-body")
                    .flex_1()
                    .min_h_0()
                    .overflow_scroll()
                    .child(body),
            )
            .into_any_element()
    }

    fn side_panel(&self, colors: &Colors) -> Option<AnyElement> {
        if self.kind != PreviewKind::Install {
            return None;
        }
        let section = |title: &'static str| {
            div()
                .text_size(u(11.0))
                .text_color(colors.text_dim)
                .child(title)
        };
        let mut panel = v_flex()
            .id("preview-side")
            .flex_none()
            .w(u(280.0))
            .h_full()
            .p(u(12.0))
            .gap(u(12.0))
            .border_l_1()
            .border_color(colors.border_variant)
            .overflow_y_scroll()
            .text_size(u(12.0));
        let hooks = if self.hooks().is_empty() {
            vec![widgets::muted("No hooks.", colors)]
        } else {
            self.hooks()
                .iter()
                .map(|h| {
                    widgets::note(
                        IconName::Zap,
                        colors.accent,
                        format!("{} {} · {}", h.kind, h.name, h.events.join(", ")),
                        colors,
                    )
                })
                .collect()
        };
        panel = panel.child(
            v_flex()
                .gap(u(4.0))
                .child(section("HOOKS THAT RUN"))
                .children(hooks),
        );
        if !self.preview.crds.is_empty() {
            panel = panel.child(
                v_flex()
                    .gap(u(4.0))
                    .child(section("CRDS (crds/)"))
                    .children(self.preview.crds.iter().map(|crd| {
                        widgets::note(
                            IconName::File,
                            colors.yellow,
                            format!("{crd}: installed once; Helm never upgrades or deletes it"),
                            colors,
                        )
                    })),
            );
        }
        if let Some(notes) = &self.preview.release.notes {
            panel = panel.child(
                v_flex().gap(u(4.0)).child(section("NOTES")).child(
                    div()
                        .font_family(fonts::MONO)
                        .text_size(u(11.5))
                        .text_color(colors.text_muted)
                        .child(SharedString::from(notes.trim().to_string())),
                ),
            );
        }
        Some(panel.into_any_element())
    }
}

impl Render for PreviewPane {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let colors = cx.colors().clone();
        let banner = self.banner(&colors);
        let warnings = self.warnings(&colors);
        let has_values = self.preview.values.is_some();
        let tabs = has_values.then(|| {
            let changes = self
                .preview
                .values
                .as_ref()
                .map(|d| d.change_count())
                .unwrap_or_default();
            h_flex()
                .flex_none()
                .h(u(34.0))
                .px(u(12.0))
                .gap(u(6.0))
                .border_b_1()
                .border_color(colors.border_variant)
                .child(widgets::toggle_chip(
                    "preview-tab-objects",
                    "Manifest",
                    self.tab == Tab::Objects,
                    cx.listener(|this, _, _, cx| {
                        this.tab = Tab::Objects;
                        cx.notify();
                    }),
                ))
                .child(widgets::toggle_chip(
                    "preview-tab-values",
                    format!("Values · {}", plural(changes, "change")),
                    self.tab == Tab::Values,
                    cx.listener(|this, _, _, cx| {
                        this.tab = Tab::Values;
                        cx.notify();
                    }),
                ))
        });
        let main = match self.tab {
            Tab::Objects => self.object_body(cx),
            Tab::Values => self.values_body(cx),
        };
        let side = self.side_panel(&colors);
        v_flex().size_full().child(banner).children(warnings).child(
            h_flex()
                .flex_1()
                .min_h_0()
                .items_start()
                .child(self.object_list(cx))
                .child(
                    v_flex()
                        .flex_1()
                        .min_w_0()
                        .h_full()
                        .overflow_hidden()
                        .children(tabs)
                        .child(main),
                )
                .children(side),
        )
    }
}

/// `1 hook`, `2 hooks`.
fn plural(n: usize, what: &str) -> String {
    if n == 1 {
        format!("1 {what}")
    } else {
        format!("{n} {what}s")
    }
}
