//! Rendering of the YAML editor (board 3): toolbar, live-change banner, the editor with its
//! gutter markers, code lenses and inline diagnostics, the bottom panel and the schema sidebar.

use gpui::{
    AnyElement, Bounds, Context, DispatchPhase, ExternalPaths, FontWeight, Hsla,
    InteractiveElement as _, IntoElement, MouseButton, MouseDownEvent, ParentElement as _, Pixels,
    SharedString, StatefulInteractiveElement as _, Styled as _, TextAlign, TextRun, Window, canvas,
    div, fill, point, prelude::FluentBuilder as _, px, rgb, size,
};
use gpui_component::input::{Editor, Input};
use kubyl_core::actions::OpenView;
use kubyl_core::{ChromeRegistry, ViewKind, ViewRequest};
use kubyl_settings::Settings;
use kubyl_ui::{
    ActiveColors, Button, Chip, Colors, Icon, IconButton, IconName, ProdBadge, fonts, h_flex,
    sizes, u, v_flex,
};

use crate::apply::Outcome;
use crate::diff::{self, LineDiff, Marker, Tag};
use crate::parse::{self, Path};
use crate::schema::Schema;
use crate::settings::YamlSettings;
use crate::templates::TEMPLATES;
use crate::validate::{Severity, Source};
use crate::view::{
    Apply, CONTEXT, DryRun, LoadState, NextProblem, PanelTab, Revert, ShowDiff, ShowHistory,
    ShowProblems, ShowTemplates, ToggleManagedFields, TogglePanel, ToggleSecrets, ToggleSideBySide,
    YamlEditor,
};

const PANEL_HEIGHT: f32 = 188.0;
const SIDEBAR_WIDTH: f32 = 300.0;
const MAX_OUTLINE_ROWS: usize = 240;

impl gpui::Render for YamlEditor {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let colors = cx.colors().clone();
        let editor_area = self.render_editor_area(window, cx);
        let mut center = v_flex()
            .flex_1()
            .h_full()
            .min_w_0()
            .min_h_0()
            .child(self.render_toolbar(cx));
        if let Some(banner) = self.render_banner(cx) {
            center = center.child(banner);
        }
        center = center.child(editor_area);
        if self.panel_open {
            center = center.child(self.render_panel(cx));
        }
        v_flex()
            .key_context(CONTEXT)
            .track_focus(self.root_focus())
            .size_full()
            .bg(colors.background)
            .text_color(colors.text)
            .on_action(cx.listener(|this, _: &Apply, window, cx| this.run(false, window, cx)))
            .on_action(cx.listener(|this, _: &DryRun, window, cx| this.run(true, window, cx)))
            .on_action(cx.listener(|this, _: &ShowDiff, _, cx| this.show_tab(PanelTab::Diff, cx)))
            .on_action(
                cx.listener(|this, _: &ShowProblems, _, cx| this.show_tab(PanelTab::Problems, cx)),
            )
            .on_action(
                cx.listener(|this, _: &ShowHistory, _, cx| this.show_tab(PanelTab::History, cx)),
            )
            .on_action(cx.listener(|this, _: &TogglePanel, _, cx| {
                this.panel_open = !this.panel_open;
                cx.notify();
            }))
            .on_action(cx.listener(|this, _: &Revert, window, cx| this.revert(window, cx)))
            .on_action(
                cx.listener(|this, _: &ToggleSecrets, window, cx| this.toggle_secrets(window, cx)),
            )
            .on_action(cx.listener(|this, _: &ToggleManagedFields, window, cx| {
                this.toggle_managed_fields(window, cx)
            }))
            .on_action(
                cx.listener(|this, _: &NextProblem, window, cx| this.next_problem(window, cx)),
            )
            .on_action(cx.listener(|_, _: &ToggleSideBySide, _, cx| {
                Settings::update::<YamlSettings>(cx, |s| {
                    s.side_by_side_diff = !s.side_by_side_diff
                });
                cx.notify();
            }))
            .on_action(cx.listener(|this, _: &ShowTemplates, _, cx| {
                if this.is_new() {
                    this.picker_open = true;
                    cx.notify();
                }
            }))
            .on_drop(cx.listener(|this, paths: &ExternalPaths, window, cx| {
                this.load_files(paths.paths().to_vec(), window, cx)
            }))
            .child(
                h_flex()
                    .flex_1()
                    .min_h_0()
                    .child(center)
                    .child(self.render_sidebar(cx)),
            )
    }
}

impl YamlEditor {
    fn show_tab(&mut self, tab: PanelTab, cx: &mut Context<Self>) {
        self.panel_tab = tab;
        self.panel_open = true;
        cx.notify();
    }

    // ----- Toolbar -----

    fn render_toolbar(&self, cx: &mut Context<Self>) -> impl IntoElement {
        let colors = cx.colors().clone();
        let caps = self.caps(cx);
        let mut crumb = h_flex()
            .min_w_0()
            .overflow_hidden()
            .whitespace_nowrap()
            .gap(u(6.0))
            .font_family(fonts::MONO)
            .text_size(u(12.0))
            .text_color(colors.text_dim)
            .child(Icon::new(IconName::File).size(13.0).color(colors.accent));
        match (&self.object, &self.gvk) {
            (Some(target), gvk) => {
                let api_version = gvk
                    .as_ref()
                    .map(|g| g.api_version())
                    .unwrap_or_else(|| target.gvr.api_version());
                let name = match &target.namespace {
                    Some(ns) => format!("{ns}/{}", target.name.clone().unwrap_or_default()),
                    None => target.name.clone().unwrap_or_default(),
                };
                crumb = crumb
                    .child(api_version)
                    .child("›")
                    .child(self.kind())
                    .child("›")
                    .child(
                        div()
                            .text_color(colors.text)
                            .font_weight(FontWeight::SEMIBOLD)
                            .child(name),
                    );
            }
            (None, gvk) => {
                crumb = crumb.child(
                    div()
                        .text_color(colors.text)
                        .font_weight(FontWeight::SEMIBOLD)
                        .child("New resource"),
                );
                if let Some(gvk) = gvk {
                    crumb = crumb.child("›").child(gvk.kind.clone());
                }
            }
        }

        // The target cluster and namespace, explicit (avoids "applied to the wrong cluster").
        let cluster_color = self
            .cluster
            .as_ref()
            .and_then(|id| {
                kubyl_kube::ConnectionManager::try_global(cx).map(|m| m.read(cx).color(id, cx))
            })
            .unwrap_or(colors.text_faint);
        let mut target_label = self.cluster_name(cx).to_string();
        if let Some(ns) = self.target_namespace() {
            target_label.push_str(&format!(" · {ns}"));
        }
        let target = h_flex()
            .flex_shrink(1.)
            .min_w_0()
            .overflow_hidden()
            .gap(u(6.0))
            .child(Chip::new(target_label).dot(cluster_color))
            .when(caps.production, |this| this.child(ProdBadge))
            .when(caps.read_only, |this| {
                this.child(Chip::new("read-only").icon(IconName::Lock))
            });

        let changes = if self.is_new() {
            let docs = parse::parse(&self.text(cx)).roots().count();
            (docs > 1).then(|| Chip::new(format!("{docs} documents")))
        } else {
            let n = self.diff.change_count();
            Some(if n == 0 {
                Chip::new("no changes").text_color(colors.text_dim)
            } else {
                Chip::new(format!(
                    "{n} change{} vs live",
                    if n == 1 { "" } else { "s" }
                ))
                .text_color(colors.yellow)
            })
        };

        let weak = self.weak();
        let busy = self.busy.is_some();
        let mut bar = h_flex()
            .h(u(sizes::TOOLBAR))
            .flex_none()
            .px(u(12.0))
            .gap(u(8.0))
            .border_b_1()
            .border_color(colors.border_variant)
            .child(crumb)
            .child(target)
            .children(changes)
            .when_some(self.busy, |this, label| {
                this.child(
                    div()
                        .text_size(u(12.0))
                        .text_color(colors.text_dim)
                        .child(label),
                )
            })
            .child(div().flex_1());
        if self.is_new() {
            bar = bar.child(
                Button::new("templates")
                    .ghost()
                    .icon(IconName::FilePlus)
                    .label("Templates")
                    .on_click(cx.listener(|this, _, _, cx| {
                        this.picker_open = true;
                        cx.notify();
                    })),
            );
        } else {
            let dirty = !self.diff.is_empty() || self.pending.is_some();
            bar = bar
                .child(
                    Button::new("revert")
                        .ghost()
                        .icon(IconName::RefreshCw)
                        .label("Revert")
                        .disabled(!dirty)
                        .on_click(cx.listener(|this, _, window, cx| this.revert(window, cx))),
                )
                .child(
                    Button::new("diff")
                        .icon(IconName::Diff)
                        .label("Diff vs live")
                        .on_click(cx.listener(|this, _, _, cx| this.show_tab(PanelTab::Diff, cx))),
                );
        }
        let weak_dry = weak.clone();
        bar.child(
            Button::new("dry-run")
                .icon(IconName::Eye)
                .label("Dry run")
                .disabled(busy)
                .on_click(move |_, window, cx| {
                    weak_dry
                        .update(cx, |this, cx| this.run(true, window, cx))
                        .ok();
                }),
        )
        .child(
            Button::new("apply")
                .primary()
                .icon(IconName::Upload)
                .label("Apply")
                .disabled(busy || caps.read_only)
                .on_click(move |_, window, cx| {
                    weak.update(cx, |this, cx| this.run(false, window, cx)).ok();
                }),
        )
    }

    // ----- Banner -----

    /// What other crates say about editing this object ([`ChromeRegistry::edit_notice`]).
    fn edit_notice(&self, cx: &Context<Self>) -> Option<SharedString> {
        let (target, live) = (self.object.as_ref()?, self.live.as_ref()?);
        ChromeRegistry::edit_notice(cx, target, live)
    }

    fn render_banner(&self, cx: &mut Context<Self>) -> Option<AnyElement> {
        let colors = cx.colors().clone();
        let (icon, color, message, actions): (IconName, Hsla, SharedString, Vec<AnyElement>) =
            if self.pending.is_some() {
                (
                    IconName::TriangleAlert,
                    colors.yellow,
                    "Object changed on server".into(),
                    vec![
                        Button::new("merge")
                            .label("Merge")
                            .on_click(
                                cx.listener(|this, _, window, cx| this.merge_pending(window, cx)),
                            )
                            .into_any_element(),
                        Button::new("reload")
                            .label("Reload")
                            .on_click(
                                cx.listener(|this, _, window, cx| this.reload_pending(window, cx)),
                            )
                            .into_any_element(),
                        Button::new("keep")
                            .ghost()
                            .label("Keep mine")
                            .on_click(cx.listener(|this, _, _, cx| this.keep_mine(cx)))
                            .into_any_element(),
                    ],
                )
            } else if self.state == LoadState::Gone {
                (
                    IconName::TriangleAlert,
                    colors.red,
                    "Deleted on the server. Apply creates it again.".into(),
                    Vec::new(),
                )
            } else if let Some(note) = self.draft_note.clone().filter(|_| self.is_new()) {
                // Where a draft's text came from (an operator's example…).
                (IconName::Info, colors.accent, note, Vec::new())
            } else {
                // Another crate's note about the object (managed by Argo CD…).
                let notice = self.edit_notice(cx)?;
                (IconName::TriangleAlert, colors.yellow, notice, Vec::new())
            };
        Some(
            h_flex()
                .flex_none()
                .h(u(34.0))
                .px(u(12.0))
                .gap(u(8.0))
                .bg(color.opacity(0.12))
                .border_b_1()
                .border_color(color.opacity(0.35))
                .text_size(u(12.5))
                .child(Icon::new(icon).size(14.0).color(color))
                .child(message)
                .child(div().flex_1())
                .children(actions)
                .into_any_element(),
        )
    }

    // ----- Editor -----

    fn render_editor_area(&mut self, window: &mut Window, cx: &mut Context<Self>) -> AnyElement {
        let colors = cx.colors().clone();
        let placeholder: Option<String> = match &self.state {
            LoadState::Waiting => Some(format!("Connecting to {}…", self.cluster_name(cx))),
            LoadState::Loading if self.text(cx).is_empty() => Some("Loading…".into()),
            LoadState::Failed(err) => Some(err.clone()),
            _ => None,
        };
        let editor = Editor::new(&self.editor)
            .h_full()
            .bordered(false)
            .font_family(fonts::MONO)
            .text_size(u(13.0))
            .bg(colors.background);
        let mut area = div()
            .relative()
            .flex_1()
            .min_h_0()
            .pt(u(6.0))
            .child(editor)
            .child(self.render_overlay(window, cx));
        if let Some(message) = placeholder {
            area = area.child(
                div()
                    .absolute()
                    .inset_0()
                    .flex()
                    .items_center()
                    .justify_center()
                    .bg(colors.background)
                    .text_color(colors.text_dim)
                    .child(message),
            );
        }
        if self.picker_open {
            area = area.child(self.render_picker(cx));
        }
        area.into_any_element()
    }

    /// Paints over the editor: change markers in the gutter, code lenses and end-of-line
    /// diagnostics. Positions come from the editor's last layout (visible lines only).
    fn render_overlay(&self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let colors = cx.colors().clone();
        let editor = self.editor.clone();
        let text = self.text(cx);
        let line_starts: Vec<usize> = std::iter::once(0)
            .chain(text.match_indices('\n').map(|(i, _)| i + 1))
            .collect();
        let line_end = |offset: usize| parse::line_range(&text, offset).end;
        let markers: Vec<(usize, Marker)> = self
            .diff
            .markers
            .iter()
            .filter_map(|(line, m)| line_starts.get(*line).map(|o| (*o, *m)))
            .collect();
        // One message per line: the most severe.
        let mut messages: Vec<(usize, Severity, String)> = Vec::new();
        for problem in self.all_problems() {
            let end = line_end(problem.range.start.min(text.len()));
            if let Some(existing) = messages.iter_mut().find(|(e, _, _)| *e == end) {
                if problem.severity < existing.1 {
                    *existing = (end, problem.severity, problem.message.clone());
                }
                continue;
            }
            messages.push((end, problem.severity, problem.message.clone()));
        }
        let mut lenses: Vec<(usize, String, bool)> = Vec::new();
        if let Some(offset) = self.lenses.metadata
            && !self.is_new()
        {
            let managers = if self.managers.is_empty() {
                "none".to_string()
            } else {
                self.managers.join(", ")
            };
            let label = if self.options.hide_managed_fields {
                format!("managedFields hidden · {managers}")
            } else {
                format!("managedFields shown · {managers}")
            };
            lenses.push((line_end(offset), label, true));
        }
        if let Some(offset) = self.lenses.status {
            lenses.push((
                line_end(offset),
                "status · read-only · live from watch".into(),
                false,
            ));
        }
        let weak = self.weak();
        canvas(
            |_, _, _| {},
            move |bounds, _, window, cx| {
                let rem = window.rem_size();
                let state = editor.read(cx);
                let Some(line_height) = state.line_height() else {
                    return;
                };
                let mut quads = Vec::new();
                for (offset, marker) in &markers {
                    let Some(at) = state.range_to_bounds(&(*offset..*offset)) else {
                        continue;
                    };
                    let color = match marker {
                        Marker::Added => colors.green,
                        Marker::Modified => colors.accent,
                        Marker::Removed => colors.red,
                    };
                    let x = at.origin.x - u(8.0).to_pixels(rem);
                    let quad = match marker {
                        Marker::Removed => fill(
                            Bounds::new(
                                point(x, at.origin.y - px(1.)),
                                size(u(8.0).to_pixels(rem), px(2.)),
                            ),
                            color,
                        ),
                        _ => fill(
                            Bounds::new(point(x, at.origin.y), size(px(3.), line_height)),
                            color,
                        ),
                    };
                    quads.push(quad);
                }
                let mut texts: Vec<OverlayText> = Vec::new();
                let mut lens_hits: Vec<Bounds<Pixels>> = Vec::new();
                let small = u(12.0).to_pixels(rem);
                let font = gpui::font(fonts::UI);
                let shape = |window: &mut Window, text: &SharedString, color: Hsla| {
                    window.text_system().shape_line(
                        text.clone(),
                        small,
                        &[TextRun {
                            len: text.len(),
                            font: font.clone(),
                            color,
                            background_color: None,
                            underline: None,
                            strikethrough: None,
                        }],
                        None,
                    )
                };
                for (offset, label, clickable) in &lenses {
                    let Some(at) = state.range_to_bounds(&(*offset..*offset)) else {
                        continue;
                    };
                    let origin = point(at.origin.x + u(24.0).to_pixels(rem), at.origin.y);
                    let label: SharedString = label.clone().into();
                    if *clickable {
                        let width = shape(window, &label, colors.text_dim).width;
                        lens_hits.push(Bounds::new(origin, size(width, line_height)));
                    }
                    texts.push((origin, label, colors.text_dim, None, false));
                }
                for (offset, severity, message) in &messages {
                    let Some(at) = state.range_to_bounds(&(*offset..*offset)) else {
                        continue;
                    };
                    let (bg, fg) = match severity {
                        Severity::Error => (rgb(0x4a2f33).into(), rgb(0xf0b4b8).into()),
                        Severity::Warning => (rgb(0x4a4230).into(), colors.yellow),
                        Severity::Info => (colors.chip_background, colors.text_muted),
                    };
                    let message: String = message.chars().take(160).collect();
                    let origin = point(at.origin.x + u(18.0).to_pixels(rem), at.origin.y);
                    texts.push((origin, message.into(), fg, Some(bg), true));
                }
                window.with_content_mask(Some(gpui::ContentMask { bounds }), |window| {
                    for quad in quads {
                        window.paint_quad(quad);
                    }
                    let pad = u(8.0).to_pixels(rem);
                    for (origin, text, color, background, pill) in texts {
                        let shaped = shape(window, &text, color);
                        let y = origin.y + (line_height - small * 1.5) / 2.;
                        if let Some(bg) = background {
                            let pill_bounds = Bounds::new(
                                point(origin.x, y),
                                size(shaped.width + pad * 2., small * 1.5),
                            );
                            window.paint_quad(fill(pill_bounds, bg).corner_radii(px(3.)));
                        }
                        let x = if pill { origin.x + pad } else { origin.x };
                        shaped
                            .paint(
                                point(x, y + small * 0.25),
                                small,
                                TextAlign::Left,
                                None,
                                window,
                                cx,
                            )
                            .ok();
                    }
                });
                if !lens_hits.is_empty() {
                    window.on_mouse_event(move |event: &MouseDownEvent, phase, window, cx| {
                        if phase != DispatchPhase::Bubble || event.button != MouseButton::Left {
                            return;
                        }
                        if lens_hits.iter().any(|b| b.contains(&event.position)) {
                            cx.stop_propagation();
                            weak.update(cx, |this, cx| this.toggle_managed_fields(window, cx))
                                .ok();
                        }
                    });
                }
            },
        )
        .absolute()
        .top_0()
        .left_0()
        .size_full()
    }

    // ----- New resource picker -----

    fn render_picker(&self, cx: &mut Context<Self>) -> impl IntoElement {
        let colors = cx.colors().clone();
        let matches = self.picker_matches(cx);
        let has_text = !self.text(cx).trim().is_empty();
        let mut target = self.cluster_name(cx).to_string();
        if let Some(ns) = &self.namespace {
            target.push_str(&format!(" · {ns}"));
        }
        let mut templates = h_flex().flex_wrap().gap(u(6.0));
        for (ix, template) in TEMPLATES.iter().enumerate() {
            templates = templates.child(
                div()
                    .id(("template", ix))
                    .cursor_pointer()
                    .on_click(
                        cx.listener(move |this, _, window, cx| this.start_template(ix, window, cx)),
                    )
                    .child(Chip::new(template.title).icon(IconName::FilePlus)),
            );
        }
        let mut list = v_flex().gap(u(1.0));
        for (ix, (gvr, gvk, namespaced)) in matches.into_iter().take(10).enumerate() {
            list = list.child(
                h_flex()
                    .id(("kind", ix))
                    .h(u(26.0))
                    .px(u(8.0))
                    .gap(u(8.0))
                    .rounded(u(4.0))
                    .cursor_pointer()
                    .hover(|s| s.bg(colors.hover))
                    .when(ix == 0, |this| this.bg(colors.selection))
                    .on_click(
                        cx.listener(move |this, _, window, cx| this.start_kind(&gvr, window, cx)),
                    )
                    .child(
                        div()
                            .font_weight(FontWeight::MEDIUM)
                            .child(gvk.kind.clone()),
                    )
                    .child(
                        div()
                            .font_family(fonts::MONO)
                            .text_size(u(11.5))
                            .text_color(colors.text_dim)
                            .child(gvk.api_version()),
                    )
                    .child(div().flex_1())
                    .child(
                        div()
                            .text_size(u(11.5))
                            .text_color(colors.text_faint)
                            .child(if namespaced { "namespaced" } else { "cluster" }),
                    ),
            );
        }
        div()
            .absolute()
            .inset_0()
            .flex()
            .justify_center()
            .pt(u(40.0))
            .bg(colors.background.opacity(0.85))
            .child(
                v_flex()
                    .w(u(520.0))
                    .max_h(u(460.0))
                    .p(u(16.0))
                    .gap(u(12.0))
                    .bg(colors.panel)
                    .border_1()
                    .border_color(colors.border)
                    .rounded(u(8.0))
                    .shadow_lg()
                    .child(
                        h_flex()
                            .gap(u(8.0))
                            .child(Icon::new(IconName::FilePlus).color(colors.accent))
                            .child(
                                div()
                                    .font_weight(FontWeight::SEMIBOLD)
                                    .child("New resource"),
                            )
                            .child(
                                div()
                                    .text_size(u(12.0))
                                    .text_color(colors.text_dim)
                                    .child(format!("in {target}")),
                            )
                            .child(div().flex_1())
                            .when(has_text, |this| {
                                this.child(IconButton::new("close-picker", IconName::X).on_click(
                                    cx.listener(|this, _, _, cx| {
                                        this.picker_open = false;
                                        cx.notify();
                                    }),
                                ))
                            }),
                    )
                    .child(templates)
                    .child(
                        div()
                            .h(u(28.0))
                            .px(u(8.0))
                            .flex()
                            .items_center()
                            .rounded(u(5.0))
                            .bg(colors.input_background)
                            .border_1()
                            .border_color(colors.border)
                            .child(Input::new(&self.picker_query).appearance(false)),
                    )
                    .child(list)
                    .child(div().text_size(u(11.5)).text_color(colors.text_dim).child(
                        "Or paste YAML (several documents separated by ---) or drop files here.",
                    )),
            )
    }

    // ----- Bottom panel -----

    fn render_panel(&self, cx: &mut Context<Self>) -> impl IntoElement {
        let colors = cx.colors().clone();
        let problems = self.all_problems().len();
        let tab = |id: &'static str,
                   icon: IconName,
                   icon_color: Hsla,
                   label: SharedString,
                   which: PanelTab,
                   badge: Option<usize>,
                   cx: &mut Context<Self>| {
            let active = self.panel_tab == which;
            h_flex()
                .id(id)
                .h_full()
                .px(u(12.0))
                .gap(u(6.0))
                .cursor_pointer()
                .text_size(u(12.5))
                .text_color(if active {
                    colors.text
                } else {
                    colors.text_muted
                })
                .when(active, |this| {
                    this.bg(colors.background)
                        .border_t_1()
                        .border_color(colors.accent)
                })
                .on_click(cx.listener(move |this, _, _, cx| this.show_tab(which, cx)))
                .child(Icon::new(icon).size(13.0).color(icon_color))
                .child(label)
                .when_some(badge.filter(|n| *n > 0), |this, n| {
                    this.child(Chip::new(n.to_string()))
                })
        };
        let side_by_side = Settings::get::<YamlSettings>(cx).side_by_side_diff;
        let mut tabs = h_flex()
            .h(u(32.0))
            .flex_none()
            .bg(colors.panel)
            .border_b_1()
            .border_color(colors.border_variant)
            .child(tab(
                "tab-diff",
                IconName::Diff,
                colors.accent,
                "Diff vs live".into(),
                PanelTab::Diff,
                None,
                cx,
            ))
            .child(tab(
                "tab-problems",
                IconName::CircleX,
                colors.red,
                "Problems".into(),
                PanelTab::Problems,
                Some(problems),
                cx,
            ))
            .child(tab(
                "tab-history",
                IconName::Clock,
                colors.text_dim,
                "Revision history".into(),
                PanelTab::History,
                None,
                cx,
            ));
        if !self.results.is_empty() {
            let label = if self.results_dry_run {
                "Dry run"
            } else {
                "Apply"
            };
            tabs = tabs.child(tab(
                "tab-results",
                IconName::CircleCheck,
                colors.green,
                label.into(),
                PanelTab::Results,
                None,
                cx,
            ));
        }
        tabs = tabs
            .child(div().flex_1())
            .when(self.panel_tab == PanelTab::Diff, |this| {
                this.child(
                    IconButton::new("side-by-side", IconName::Columns)
                        .toggled(side_by_side)
                        .on_click(|_, window, cx| {
                            window.dispatch_action(Box::new(ToggleSideBySide), cx)
                        }),
                )
            })
            .child(
                IconButton::new("close-panel", IconName::X).on_click(cx.listener(
                    |this, _, _, cx| {
                        this.panel_open = false;
                        cx.notify();
                    },
                )),
            );
        let body = match self.panel_tab {
            PanelTab::Diff => self.render_diff(side_by_side, cx),
            PanelTab::Problems => self.render_problems(cx),
            PanelTab::History => self.render_history(cx),
            PanelTab::Results => self.render_results(cx),
        };
        v_flex()
            .h(u(PANEL_HEIGHT))
            .flex_none()
            .border_t_1()
            .border_color(colors.border)
            .child(tabs)
            .child(
                div()
                    .id("panel-body")
                    .flex_1()
                    .min_h_0()
                    .overflow_y_scroll()
                    .py(u(6.0))
                    .child(body),
            )
    }

    fn empty(message: impl Into<SharedString>, colors: &Colors) -> AnyElement {
        div()
            .px(u(14.0))
            .py(u(8.0))
            .text_size(u(12.5))
            .text_color(colors.text_dim)
            .child(message.into())
            .into_any_element()
    }

    fn render_diff(&self, side_by_side: bool, cx: &mut Context<Self>) -> AnyElement {
        let colors = cx.colors().clone();
        if self.is_new() {
            return Self::empty(
                "A new resource: nothing to compare yet. Dry run validates it against the server.",
                &colors,
            );
        }
        if self.diff.is_empty() {
            return Self::empty("No changes vs live.", &colors);
        }
        diff_view(&self.diff, side_by_side, &colors)
    }

    fn render_problems(&self, cx: &mut Context<Self>) -> AnyElement {
        let colors = cx.colors().clone();
        let problems = self.all_problems();
        if problems.is_empty() {
            let message = if Settings::get::<YamlSettings>(cx).validate {
                "No problems."
            } else {
                "No problems. Schema validation is off."
            };
            return Self::empty(message, &colors);
        }
        let text = self.text(cx);
        let mut list = v_flex();
        for (ix, problem) in problems.into_iter().enumerate() {
            let (icon, color) = match problem.severity {
                Severity::Error => (IconName::CircleX, colors.red),
                Severity::Warning => (IconName::TriangleAlert, colors.yellow),
                Severity::Info => (IconName::Info, colors.text_dim),
            };
            let line = parse::line_of(&text, problem.range.start) + 1;
            let source = match problem.source {
                Source::Syntax => "yaml",
                Source::Schema => "schema",
                Source::Server => "server",
            };
            let offset = problem.range.start;
            list = list.child(
                h_flex()
                    .id(("problem", ix))
                    .px(u(14.0))
                    .py(u(3.0))
                    .gap(u(8.0))
                    .text_size(u(12.5))
                    .cursor_pointer()
                    .hover(|s| s.bg(colors.hover))
                    .on_click(
                        cx.listener(move |this, _, window, cx| this.jump_to(offset, window, cx)),
                    )
                    .child(Icon::new(icon).size(13.0).color(color))
                    .child(
                        div()
                            .flex_1()
                            .min_w_0()
                            .truncate()
                            .child(problem.message.clone()),
                    )
                    .when_some(problem.path.clone(), |this, path| {
                        this.child(
                            div()
                                .font_family(fonts::MONO)
                                .text_size(u(11.5))
                                .text_color(colors.text_dim)
                                .child(path),
                        )
                    })
                    .child(
                        div()
                            .text_size(u(11.5))
                            .text_color(colors.text_faint)
                            .child(format!("{source} · Ln {line}")),
                    ),
            );
        }
        list.into_any_element()
    }

    fn render_history(&self, cx: &mut Context<Self>) -> AnyElement {
        let colors = cx.colors().clone();
        if self.is_new() {
            return Self::empty("Revisions appear once the object exists.", &colors);
        }
        let now = jiff::Timestamp::now();
        let age = |ts: Option<&str>| {
            ts.and_then(kubyl_resources::format::timestamp)
                .map(|t| {
                    kubyl_resources::format::human_duration(kubyl_resources::format::seconds_since(
                        t, now,
                    ))
                })
                .unwrap_or_default()
        };
        let mut list = v_flex();
        if self.has_workload_history() {
            match &self.revisions {
                None => return Self::empty("Loading revisions…", &colors),
                Some(Err(err)) => return Self::empty(err.clone(), &colors),
                Some(Ok(revisions)) if revisions.is_empty() => {
                    return Self::empty("No revisions.", &colors);
                }
                Some(Ok(revisions)) => {
                    for (ix, revision) in revisions.iter().enumerate() {
                        let number = revision.revision;
                        list = list.child(
                            h_flex()
                                .id(("revision", ix))
                                .px(u(14.0))
                                .h(u(26.0))
                                .gap(u(10.0))
                                .text_size(u(12.5))
                                .hover(|s| s.bg(colors.hover))
                                .child(
                                    div()
                                        .w(u(44.0))
                                        .font_family(fonts::MONO)
                                        .child(format!("#{number}")),
                                )
                                .child(if revision.current {
                                    Chip::new("current")
                                        .text_color(colors.green)
                                        .into_any_element()
                                } else {
                                    div().w(u(58.0)).into_any_element()
                                })
                                .child(
                                    div()
                                        .flex_1()
                                        .min_w_0()
                                        .truncate()
                                        .font_family(fonts::MONO)
                                        .text_size(u(11.5))
                                        .text_color(colors.text_muted)
                                        .child(revision.images.join(", ")),
                                )
                                .when_some(revision.change_cause.clone(), |this, cause| {
                                    this.child(
                                        div()
                                            .max_w(u(220.0))
                                            .truncate()
                                            .text_color(colors.text_dim)
                                            .child(cause),
                                    )
                                })
                                .child(
                                    div()
                                        .w(u(40.0))
                                        .text_color(colors.text_dim)
                                        .child(age(revision.created.as_deref())),
                                )
                                .when(!revision.current, |this| {
                                    this.child(
                                        Button::new(("rollback", ix))
                                            .ghost()
                                            .label("Roll back")
                                            .on_click(cx.listener(move |this, _, window, cx| {
                                                this.roll_back(number, window, cx)
                                            })),
                                    )
                                }),
                        );
                    }
                }
            }
            return list.into_any_element();
        }
        if self.local_history.is_empty() {
            return Self::empty(
                "No applies from Kubyl yet. Kubyl keeps the last versions it applied (never Secrets).",
                &colors,
            );
        }
        let buffer = self.text(cx);
        for (ix, entry) in self.local_history.iter().enumerate() {
            let changes = diff::diff(&entry.yaml, &buffer, 0).change_count();
            list = list.child(
                h_flex()
                    .id(("local", ix))
                    .px(u(14.0))
                    .h(u(26.0))
                    .gap(u(10.0))
                    .text_size(u(12.5))
                    .hover(|s| s.bg(colors.hover))
                    .child(Icon::new(IconName::Clock).size(13.0))
                    .child(div().child(format!("Applied {} ago", age(Some(&entry.applied_at)))))
                    .child(
                        div()
                            .flex_1()
                            .text_color(colors.text_dim)
                            .child(if changes == 0 {
                                "same as the buffer".to_string()
                            } else {
                                format!("{changes} lines differ from the buffer")
                            }),
                    )
                    .child(
                        Button::new(("restore", ix))
                            .ghost()
                            .label("Load into editor")
                            .disabled(changes == 0)
                            .on_click(cx.listener(move |this, _, window, cx| {
                                this.restore_history(ix, window, cx)
                            })),
                    ),
            );
        }
        list.into_any_element()
    }

    fn render_results(&self, cx: &mut Context<Self>) -> AnyElement {
        let colors = cx.colors().clone();
        let mut list = v_flex();
        for (ix, result) in self.results.iter().enumerate() {
            let (icon, color, message) = match &result.outcome {
                Outcome::Applied(_) => (
                    IconName::CircleCheck,
                    colors.green,
                    if self.results_dry_run {
                        "valid (dry run)".to_string()
                    } else {
                        "applied".to_string()
                    },
                ),
                Outcome::Conflicts(c) => {
                    let mut managers: Vec<&str> = c.iter().map(|c| c.manager.as_str()).collect();
                    managers.sort_unstable();
                    managers.dedup();
                    (
                        IconName::TriangleAlert,
                        colors.yellow,
                        format!("conflicts with {}", managers.join(", ")),
                    )
                }
                Outcome::Failed { message, .. } => (IconName::CircleX, colors.red, message.clone()),
            };
            let offset = result.span.start;
            list = list.child(
                h_flex()
                    .id(("result", ix))
                    .px(u(14.0))
                    .py(u(3.0))
                    .gap(u(8.0))
                    .text_size(u(12.5))
                    .cursor_pointer()
                    .hover(|s| s.bg(colors.hover))
                    .on_click(
                        cx.listener(move |this, _, window, cx| this.jump_to(offset, window, cx)),
                    )
                    .child(Icon::new(icon).size(13.0).color(color))
                    .child(
                        div()
                            .w(u(34.0))
                            .text_color(colors.text_faint)
                            .child(format!("#{}", result.index + 1)),
                    )
                    .child(div().font_family(fonts::MONO).child(result.label.clone()))
                    .child(
                        div()
                            .flex_1()
                            .min_w_0()
                            .truncate()
                            .text_color(colors.text_muted)
                            .child(message),
                    ),
            );
        }
        list.into_any_element()
    }

    // ----- Sidebar -----

    fn render_sidebar(&mut self, cx: &mut Context<Self>) -> impl IntoElement {
        let colors = cx.colors().clone();
        let gvk = self.gvk.clone();
        let schema_doc = gvk.as_ref().and_then(|g| self.schema_lookup(g, cx));
        let source = match (&gvk, &schema_doc) {
            (Some(g), Some(_)) if crate::schema::is_builtin_group(&g.group) => "built-in",
            (Some(_), Some(_)) => "from CRD",
            (Some(_), None) if self.cluster.is_some() => "loading…",
            _ => "",
        };
        let mut outline = v_flex().py(u(6.0));
        match (&gvk, &schema_doc) {
            (Some(gvk), Some((doc, _))) => match Schema::for_gvk(doc, gvk) {
                Some(root) => {
                    let mut rows = Vec::new();
                    self.outline_rows(&root, &Path::default(), 0, &mut rows);
                    for (ix, row) in rows.into_iter().enumerate() {
                        outline = outline.child(self.render_outline_row(ix, row, &colors, cx));
                    }
                }
                None => {
                    outline = outline.child(Self::empty("No schema for this kind.", &colors));
                }
            },
            (None, _) => {
                outline = outline.child(Self::empty("Pick a kind to see its schema.", &colors));
            }
            _ => {}
        }

        let settings = Settings::get::<YamlSettings>(cx).clone();
        let pill = |id: &'static str, icon: IconName, color: Hsla, label: String| {
            h_flex()
                .id(id)
                .gap(u(6.0))
                .px(u(8.0))
                .h(u(22.0))
                .rounded(u(11.0))
                .bg(colors.chip_background)
                .cursor_pointer()
                .hover(|s| s.bg(colors.hover))
                .text_size(u(12.0))
                .text_color(colors.text_muted)
                .child(Icon::new(icon).size(12.0).color(color))
                .child(label)
        };
        let mut editor_section = v_flex().items_start().gap(u(6.0)).child(
            pill(
                "pill-validate",
                if settings.validate {
                    IconName::CircleCheck
                } else {
                    IconName::CircleX
                },
                if settings.validate {
                    colors.green
                } else {
                    colors.text_dim
                },
                format!(
                    "Schema validation {}",
                    if settings.validate { "on" } else { "off" }
                ),
            )
            .on_click(cx.listener(|this, _, _, cx| {
                Settings::update::<YamlSettings>(cx, |s| s.validate = !s.validate);
                this.schedule_analysis(cx);
            })),
        );
        if !self.is_new() {
            editor_section = editor_section.child(
                pill(
                    "pill-managed",
                    IconName::Eye,
                    colors.text_dim,
                    if self.options.hide_managed_fields {
                        "Hide managedFields".into()
                    } else {
                        "Show managedFields".into()
                    },
                )
                .on_click(
                    cx.listener(|this, _, window, cx| this.toggle_managed_fields(window, cx)),
                ),
            );
        }
        if self.secret.is_some() {
            editor_section = editor_section.child(
                pill(
                    "pill-secrets",
                    IconName::Key,
                    colors.yellow,
                    if self.options.reveal_secrets {
                        "Mask secret values".into()
                    } else {
                        "Reveal secret values".into()
                    },
                )
                .on_click(cx.listener(|this, _, window, cx| this.toggle_secrets(window, cx))),
            );
        }
        editor_section = editor_section.child(
            pill(
                "pill-prod",
                IconName::Shield,
                if settings.confirm_apply_on_prod {
                    colors.yellow
                } else {
                    colors.text_dim
                },
                format!(
                    "Confirm apply on PROD{}",
                    if settings.confirm_apply_on_prod {
                        ""
                    } else {
                        " (off)"
                    }
                ),
            )
            .on_click(cx.listener(|_, _, _, cx| {
                Settings::update::<YamlSettings>(cx, |s| {
                    s.confirm_apply_on_prod = !s.confirm_apply_on_prod
                });
                cx.notify();
            })),
        );

        let section_title = |title: &'static str| {
            div()
                .text_size(u(11.0))
                .font_weight(FontWeight::SEMIBOLD)
                .text_color(colors.text_dim)
                .mb(u(6.0))
                .child(title.to_uppercase())
        };
        let mut related = v_flex().gap(u(2.0));
        for (ix, item) in self.related.iter().enumerate() {
            let icon = match item.relation {
                "Secret" | "Pull secret" | "TLS secret" => IconName::Key,
                "Issuer" => IconName::Shield,
                _ => IconName::Link,
            };
            let target = item.target.clone();
            related = related.child(
                h_flex()
                    .id(("related", ix))
                    .gap(u(8.0))
                    .py(u(3.0))
                    .text_size(u(12.0))
                    .when(target.is_some(), |this| this.cursor_pointer())
                    .on_click(move |_, window, cx| {
                        if let Some(target) = target.clone() {
                            window.dispatch_action(
                                Box::new(OpenView(ViewRequest::for_resource(
                                    ViewKind::Details,
                                    target,
                                ))),
                                cx,
                            );
                        }
                    })
                    .child(Icon::new(icon).size(13.0).color(colors.text_dim))
                    .child(
                        div()
                            .font_family(fonts::MONO)
                            .text_size(u(11.5))
                            .text_color(colors.accent)
                            .truncate()
                            .child(item.title.to_lowercase().replacen(' ', "/", 1)),
                    )
                    .child(div().text_color(colors.text_dim).child(item.relation)),
            );
        }

        v_flex()
            .w(u(SIDEBAR_WIDTH))
            .flex_none()
            .h_full()
            .bg(colors.panel)
            .border_l_1()
            .border_color(colors.border)
            .child(
                h_flex()
                    .h(u(sizes::PANEL_HEADER))
                    .flex_none()
                    .px(u(12.0))
                    .border_b_1()
                    .border_color(colors.border_variant)
                    .child(
                        div()
                            .flex_1()
                            .font_weight(FontWeight::MEDIUM)
                            .child("Schema"),
                    )
                    .child(
                        div()
                            .text_size(u(11.5))
                            .text_color(colors.text_dim)
                            .child(source),
                    ),
            )
            .child(
                div()
                    .id("outline")
                    .flex_1()
                    .min_h_0()
                    .overflow_y_scroll()
                    .child(outline),
            )
            .when(!self.related.is_empty(), |this| {
                this.child(
                    v_flex()
                        .flex_none()
                        .px(u(12.0))
                        .py(u(10.0))
                        .border_t_1()
                        .border_color(colors.border_variant)
                        .child(section_title("Related objects"))
                        .child(related),
                )
            })
            .child(
                v_flex()
                    .flex_none()
                    .px(u(12.0))
                    .py(u(10.0))
                    .border_t_1()
                    .border_color(colors.border_variant)
                    .child(section_title("Editor"))
                    .child(editor_section),
            )
    }

    fn outline_rows<'a>(
        &self,
        schema: &Schema<'a>,
        path: &Path,
        depth: usize,
        rows: &mut Vec<OutlineRow>,
    ) {
        let required = schema.required();
        for (name, child) in schema.properties() {
            if rows.len() >= MAX_OUTLINE_ROWS {
                return;
            }
            if depth == 0 && matches!(name, "apiVersion" | "kind" | "status") {
                continue;
            }
            let child_path = path.push_key(name);
            let key = child_path.to_string();
            // Arrays of objects expand into their item's fields.
            let (expandable_schema, suffix) = match child.type_name() {
                Some("array") => (child.items().filter(|i| !i.properties().is_empty()), true),
                _ => (
                    Some(child.clone()).filter(|c| !c.properties().is_empty()),
                    false,
                ),
            };
            let expanded = self.expanded.contains(&key);
            rows.push(OutlineRow {
                path: child_path.clone(),
                name: name.to_string(),
                type_label: child.type_label(),
                depth,
                required: required.contains(&name),
                expandable: expandable_schema.is_some(),
                expanded,
            });
            if expanded && let Some(inner) = expandable_schema {
                let inner_path = if suffix {
                    child_path.push_index(0)
                } else {
                    child_path
                };
                self.outline_rows(&inner, &inner_path, depth + 1, rows);
            }
        }
    }

    fn render_outline_row(
        &self,
        ix: usize,
        row: OutlineRow,
        colors: &Colors,
        cx: &mut Context<Self>,
    ) -> impl IntoElement {
        let key = row.path.to_string();
        let display = {
            let mut p = row.path.clone();
            while matches!(p.0.last(), Some(parse::Seg::Index(_))) {
                p.0.pop();
            }
            p.to_string().replace("[0]", "")
        };
        let on = self
            .cursor_path
            .as_deref()
            .map(|p| p.replace(|c: char| c == '[' || c.is_ascii_digit() || c == ']', ""))
            == Some(display);
        let path = row.path.clone();
        let expandable = row.expandable;
        h_flex()
            .id(("field", ix))
            .h(u(sizes::TREE_ROW))
            .pl(u(12.0 + row.depth as f32 * 14.0))
            .pr(u(12.0))
            .gap(u(4.0))
            .cursor_pointer()
            .when(on, |this| this.bg(colors.selection))
            .hover(|s| s.bg(colors.hover))
            .on_click(cx.listener(move |this, _, window, cx| {
                if expandable && !this.expanded.remove(&key) {
                    this.expanded.insert(key.clone());
                }
                let target = Path(
                    path.0
                        .iter()
                        .filter(|s| matches!(s, parse::Seg::Key(_)))
                        .cloned()
                        .collect(),
                );
                if path.0.iter().all(|s| matches!(s, parse::Seg::Key(_))) {
                    this.go_to_field(&target, window, cx);
                }
                cx.notify();
            }))
            .child(div().w(u(12.0)).when(row.expandable, |this| {
                this.child(
                    Icon::new(if row.expanded {
                        IconName::ChevronDown
                    } else {
                        IconName::ChevronRight
                    })
                    .size(11.0)
                    .color(colors.text_faint),
                )
            }))
            .child(
                h_flex()
                    .flex_1()
                    .min_w_0()
                    .font_family(fonts::MONO)
                    .text_size(u(12.0))
                    .text_color(if on { colors.text } else { colors.text_muted })
                    .child(div().truncate().child(row.name))
                    .when(row.required, |this| {
                        this.child(div().text_color(colors.red).child(" *"))
                    }),
            )
            .child(
                div()
                    .flex_none()
                    .font_family(fonts::MONO)
                    .text_size(u(11.0))
                    .text_color(colors.cyan)
                    .child(row.type_label),
            )
    }
}

/// Text the overlay paints: origin, text, color, pill background, and whether it's a pill.
type OverlayText = (gpui::Point<Pixels>, SharedString, Hsla, Option<Hsla>, bool);

struct OutlineRow {
    path: Path,
    name: String,
    type_label: String,
    depth: usize,
    required: bool,
    expandable: bool,
    expanded: bool,
}

/// A diff's hunks as the editor's Diff panel shows them (unified or side by side), for other
/// views that compare two renderings of an object (Argo CD's desired vs live).
pub fn diff_view(line_diff: &LineDiff, side_by_side: bool, colors: &Colors) -> AnyElement {
    let removed_bg: Hsla = rgb(0x3a2e31).into();
    let removed_fg: Hsla = rgb(0xe7a9ad).into();
    let added_bg: Hsla = rgb(0x2d3a2c).into();
    let added_fg: Hsla = rgb(0xbfd9a6).into();
    let mut out = v_flex()
        .font_family(fonts::MONO)
        .text_size(u(12.5))
        .line_height(u(21.0));
    for hunk in &line_diff.hunks {
        out = out.child(
            div()
                .px(u(14.0))
                .text_color(colors.text_dim)
                .child(format!("@@ {} @@", hunk.section)),
        );
        if side_by_side {
            for row in diff::side_by_side(hunk) {
                let cell = |side: Option<(usize, String)>, bg: Hsla, fg: Hsla| {
                    div()
                        .flex_1()
                        .min_w_0()
                        .px(u(14.0))
                        .whitespace_nowrap()
                        .overflow_hidden()
                        .when(row.changed && side.is_some(), |this| {
                            this.bg(bg).text_color(fg)
                        })
                        .when(!row.changed, |this| this.text_color(colors.text_muted))
                        .child(side.map(|(_, t)| t).unwrap_or_default())
                };
                out = out.child(
                    h_flex()
                        .child(cell(row.left.clone(), removed_bg, removed_fg))
                        .child(div().w(px(1.)).h_full().bg(colors.border_variant))
                        .child(cell(row.right.clone(), added_bg, added_fg)),
                );
            }
        } else {
            for line in &hunk.lines {
                let (sign, bg, fg) = match line.tag {
                    Tag::Removed => ("-", Some(removed_bg), removed_fg),
                    Tag::Added => ("+", Some(added_bg), added_fg),
                    Tag::Equal => (" ", None, colors.text_muted),
                };
                out = out.child(
                    div()
                        .px(u(14.0))
                        .whitespace_nowrap()
                        .text_color(fg)
                        .when_some(bg, |this, bg| this.bg(bg))
                        .child(format!("{sign} {}", line.text)),
                );
            }
        }
    }
    out.into_any_element()
}
