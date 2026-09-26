//! Rendering of the editor tab (board 11): toolbar, changed-on-disk banner, the list of
//! contexts, clusters and users, the form or YAML tab, and the test panel.

use std::rc::Rc;

use gpui::{
    AnyElement, Context, FontWeight, IntoElement, Render, SharedString, Window, div, prelude::*,
};
use gpui_component::button::{Button as MenuButton, ButtonVariants as _};
use gpui_component::input::Editor;
use gpui_component::menu::{DropdownMenu as _, PopupMenuItem};
use kubyl_kube::ConnectionManager;
use kubyl_kube::settings::display_path;
use kubyl_settings::Settings;
use kubyl_ui::{
    ActiveColors, Button, Chip, Colors, Icon, IconName, KeyHints, fonts, h_flex, sizes, u, v_flex,
};

use crate::editor::{
    CONTEXT, KubeconfigEditor, Revert, Save, ShowForm, ShowYaml, Tab, TestAll, TestConnection,
    ToggleSecrets,
};
use crate::model::{AuthKind, EntryRef, Kind};
use crate::panel;
use crate::settings::KubeconfigSettings;
use crate::state::{Kubeconfigs, TestKey};
use crate::validate::{self, Severity};
use crate::widgets;

impl KubeconfigEditor {
    fn render_toolbar(&self, cx: &mut Context<Self>) -> impl IntoElement {
        let colors = cx.colors().clone();
        let weak = cx.entity().downgrade();
        let settings = Settings::get::<KubeconfigSettings>(cx).clone();
        let owned = self.is_owned();
        let editable = self.is_editable(cx);
        let (badge_color, badge) = if self.is_draft() {
            (colors.accent, "New · Kubyl-owned".to_string())
        } else if owned {
            (colors.green, "Kubyl-owned".to_string())
        } else if editable {
            (colors.green, "External file · editing on".to_string())
        } else {
            (colors.text_faint, "External file · read-only".to_string())
        };
        let count = self.change_count;
        let changes = (!self.is_draft() && self.is_dirty() && count > 0).then(|| {
            if count == 1 {
                "1 unsaved change".to_string()
            } else {
                format!("{count} unsaved changes")
            }
        });
        let tab = self.tab;
        let seg = {
            let weak = weak.clone();
            widgets::segmented(
                "kc-tab",
                vec![
                    (
                        Tab::Form,
                        SharedString::from("Form"),
                        Some(IconName::SlidersVertical),
                    ),
                    (Tab::Yaml, "YAML".into(), Some(IconName::Code)),
                ],
                tab,
                &colors,
                move |tab, window, cx| {
                    let tab = *tab;
                    weak.update(cx, |this, cx| this.set_tab(tab, window, cx))
                        .ok();
                },
            )
        };
        let context = self.selected_context();
        let more = {
            let weak = weak.clone();
            let can_opt_in = !owned && settings.allow_external_edits && !self.is_draft();
            let opted_in = !owned && editable;
            MenuButton::new("kc-more")
                .ghost()
                .compact()
                .child(Icon::new(IconName::Ellipsis).size(14.0))
                .dropdown_menu(move |menu, _, _| {
                    let mut menu = menu;
                    let w = weak.clone();
                    menu = menu.item(
                        PopupMenuItem::new("Merge another kubeconfig into this one…").on_click(
                            move |_, window, cx| crate::dialogs::merge_into(w.clone(), window, cx),
                        ),
                    );
                    let w = weak.clone();
                    menu = menu.item(
                        PopupMenuItem::new("Split into one file per context…").on_click(
                            move |_, window, cx| crate::dialogs::split(w.clone(), window, cx),
                        ),
                    );
                    if !owned {
                        let w = weak.clone();
                        menu = menu.item(PopupMenuItem::new("Save as a Kubyl copy…").on_click(
                            move |_, window, cx| crate::dialogs::save_copy(w.clone(), window, cx),
                        ));
                    }
                    if can_opt_in && !opted_in {
                        let w = weak.clone();
                        menu =
                            menu.separator()
                                .item(PopupMenuItem::new("Edit this file…").on_click(
                                    move |_, window, cx| {
                                        crate::dialogs::opt_in(w.clone(), window, cx)
                                    },
                                ));
                    }
                    if opted_in {
                        let w = weak.clone();
                        menu = menu.separator().item(
                            PopupMenuItem::new("Stop editing this file").on_click(
                                move |_, _, cx| {
                                    if let Some(this) = w.upgrade() {
                                        let path = this.read(cx).path.clone();
                                        crate::settings::set_opt_in(&path, false, cx);
                                    }
                                },
                            ),
                        );
                    }
                    menu.separator()
                        .item(
                            PopupMenuItem::new("Open the backups folder").on_click(|_, _, _| {
                                let dir = crate::files::backup_dir();
                                std::fs::create_dir_all(&dir).ok();
                                open::that_detached(dir).ok();
                            }),
                        )
                })
        };
        h_flex()
            .h(u(40.0))
            .flex_none()
            .px(u(12.0))
            .gap(u(8.0))
            .border_b_1()
            .border_color(colors.border_variant)
            .child(Icon::new(IconName::File).size(14.0).color(colors.text_dim))
            .child(
                div()
                    .font_family(fonts::MONO)
                    .text_size(u(12.5))
                    .max_w(u(360.0))
                    .truncate()
                    .child(display_path(&self.path)),
            )
            .child(Chip::new(badge).dot(badge_color))
            .children(changes.map(|c| Chip::new(c).selected(true)))
            .child(div().flex_1())
            .child(div().w(u(170.0)).child(seg))
            .child({
                let weak = weak.clone();
                Button::new("kc-test")
                    .icon(IconName::Play)
                    .label("Test connection")
                    .disabled(context.is_none())
                    .on_click(move |_, window, cx| {
                        weak.update(cx, |this, cx| {
                            if let Some(context) = this.selected_context() {
                                this.test(context, window, cx);
                            }
                        })
                        .ok();
                    })
            })
            .child({
                let weak = weak.clone();
                Button::new("kc-test-all")
                    .label("Test all")
                    .disabled(self.doc.names(Kind::Context).is_empty())
                    .on_click(move |_, window, cx| {
                        weak.update(cx, |this, cx| this.test_all(window, cx)).ok();
                    })
            })
            .when(!self.is_draft(), |this| {
                let weak = weak.clone();
                this.child(
                    Button::new("kc-revert")
                        .ghost()
                        .icon(IconName::RotateCcw)
                        .label("Revert")
                        .disabled(!self.is_dirty())
                        .on_click(move |_, window, cx| {
                            weak.update(cx, |this, cx| this.revert(window, cx)).ok();
                        }),
                )
            })
            .child({
                let weak = weak.clone();
                Button::new("kc-save")
                    .primary()
                    .label(if self.saving { "Saving…" } else { "Save…" })
                    .disabled(self.saving || !self.is_dirty())
                    .on_click(move |_, window, cx| {
                        crate::dialogs::save_preview(weak.clone(), window, cx)
                    })
            })
            .child(more)
    }

    /// The selected context, or the first context that uses the selected cluster or user.
    pub(crate) fn selected_context(&self) -> Option<String> {
        let entry = self.selection.as_ref()?;
        match entry.kind {
            Kind::Context => Some(entry.name.clone()),
            kind => self.doc.used_by(kind, &entry.name).into_iter().next(),
        }
    }

    fn render_banner(&self, cx: &mut Context<Self>) -> Option<AnyElement> {
        let colors = cx.colors().clone();
        self.disk.as_ref()?;
        let weak = cx.entity().downgrade();
        Some(
            h_flex()
                .px(u(12.0))
                .py(u(8.0))
                .gap(u(10.0))
                .border_b_1()
                .border_color(colors.yellow.opacity(0.4))
                .bg(colors.yellow.opacity(0.08))
                .child(Icon::new(IconName::TriangleAlert).size(14.0).color(colors.yellow))
                .child(
                    div()
                        .flex_1()
                        .text_size(u(12.5))
                        .child(format!(
                            "{} changed on disk. Your unsaved changes are kept; Kubyl won't overwrite the file without asking.",
                            crate::editor::file_title(&self.path)
                        )),
                )
                .child({
                    let weak = weak.clone();
                    Button::new("kc-disk-diff")
                        .icon(IconName::Diff)
                        .label("Show diff")
                        .on_click(move |_, window, cx| crate::dialogs::disk_diff(weak.clone(), window, cx))
                })
                .child({
                    let weak = weak.clone();
                    Button::new("kc-disk-reload")
                        .icon(IconName::RefreshCw)
                        .label("Reload")
                        .on_click(move |_, window, cx| {
                            weak.update(cx, |this, cx| this.reload_from_disk(window, cx)).ok();
                        })
                })
                .child(
                    Button::new("kc-disk-keep")
                        .ghost()
                        .label("Keep mine")
                        .on_click(move |_, window, cx| crate::dialogs::keep_mine(weak.clone(), window, cx)),
                )
                .into_any_element(),
        )
    }

    fn render_list(&self, cx: &mut Context<Self>) -> impl IntoElement {
        let colors = cx.colors().clone();
        let weak = cx.entity().downgrade();
        let filter = self.filter.read(cx).value().to_lowercase();
        let global = Kubeconfigs::try_global(cx);
        let doc_key = self.doc_key();
        let manager = ConnectionManager::try_global(cx);
        let mut list = v_flex().gap(u(2.0));
        for kind in Kind::ALL {
            let names: Vec<String> = self
                .doc
                .names(kind)
                .into_iter()
                .filter(|n| filter.is_empty() || n.to_lowercase().contains(&filter))
                .collect();
            let add = {
                let weak = weak.clone();
                kubyl_ui::IconButton::new(
                    SharedString::from(format!("kc-add-{}", kind.label())),
                    IconName::Plus,
                )
                .icon_size(13.0)
                .on_click(move |_, window, cx| {
                    weak.update(cx, |this, cx| this.add(kind, window, cx)).ok();
                })
            };
            list = list.child(
                h_flex()
                    .h(u(26.0))
                    .mt(u(6.0))
                    .px(u(10.0))
                    .gap(u(6.0))
                    .child(
                        div()
                            .text_size(u(11.0))
                            .font_weight(FontWeight::SEMIBOLD)
                            .text_color(colors.text_dim)
                            .child(kind.title().to_uppercase()),
                    )
                    .child(
                        div()
                            .text_size(u(11.0))
                            .text_color(colors.text_faint)
                            .child(names.len().to_string()),
                    )
                    .child(div().flex_1())
                    .child(add),
            );
            for name in names {
                let entry = EntryRef::new(kind, name.clone());
                let selected = self.selection.as_ref() == Some(&entry);
                let secondary = self.secondary_label(&entry);
                let problem = validate::worst(&self.problems, &entry);
                let report = match kind {
                    Kind::Context => global.as_ref().and_then(|g| {
                        g.read(cx)
                            .report(&TestKey::new(doc_key.clone(), name.clone()))
                            .cloned()
                    }),
                    _ => None,
                };
                let dot = match (kind, &manager) {
                    (Kind::Context, Some(m)) => self
                        .cluster_id(&name, cx)
                        .map(|id| m.read(cx).color(&id, cx))
                        .unwrap_or(colors.text_faint),
                    _ => colors.text_faint,
                };
                let icon = match kind {
                    Kind::Context => None,
                    Kind::Cluster => Some(IconName::Server),
                    Kind::User => Some(IconName::Key),
                };
                let weak = weak.clone();
                let click = entry.clone();
                list = list.child(
                    h_flex()
                        .id(SharedString::from(format!(
                            "kc-row-{}-{name}",
                            kind.label()
                        )))
                        .h(u(26.0))
                        .px(u(10.0))
                        .gap(u(7.0))
                        .cursor_pointer()
                        .map(|this| {
                            if selected {
                                this.bg(colors.selection)
                                    .border_1()
                                    .border_color(colors.accent)
                            } else {
                                this.border_1()
                                    .border_color(gpui::transparent_black())
                                    .hover(|s| s.bg(colors.hover))
                            }
                        })
                        .on_click(move |_, window, cx| {
                            let click = click.clone();
                            weak.update(cx, |this, cx| this.select(Some(click), window, cx))
                                .ok();
                        })
                        .child(match icon {
                            Some(icon) => Icon::new(icon)
                                .size(12.0)
                                .color(colors.text_dim)
                                .into_any_element(),
                            None => div().size(u(7.0)).rounded_full().bg(dot).into_any_element(),
                        })
                        .child(
                            div()
                                .text_size(u(12.5))
                                .flex_none()
                                .max_w(u(150.0))
                                .truncate()
                                .text_color(if selected {
                                    colors.text
                                } else {
                                    colors.text_muted
                                })
                                .child(name.clone()),
                        )
                        .child(
                            div()
                                .flex_1()
                                .min_w_0()
                                .truncate()
                                .text_size(u(11.0))
                                .text_color(colors.text_faint)
                                .child(secondary),
                        )
                        .children(problem.filter(|s| *s != Severity::Info).map(|s| {
                            let (color, icon) = widgets::severity_style(s, &colors);
                            Icon::new(icon).size(12.0).color(color)
                        }))
                        .children(report.map(|r| panel::badge(&r, &colors))),
                );
            }
        }
        v_flex()
            .w(u(290.0))
            .flex_none()
            .h_full()
            .bg(colors.panel)
            .border_r_1()
            .border_color(colors.border)
            .child(
                div()
                    .p(u(8.0))
                    .child(widgets::text_input(&self.filter, false, false, &colors)),
            )
            .child(
                div()
                    .id("kc-list")
                    .flex_1()
                    .overflow_y_scroll()
                    .pb(u(8.0))
                    .child(list),
            )
            .child(
                div()
                    .p(u(10.0))
                    .border_t_1()
                    .border_color(colors.border_variant)
                    .text_size(u(11.0))
                    .text_color(colors.text_dim)
                    .child(format!(
                        "Comments and key order are kept. Backups: {}",
                        display_path(&crate::files::backup_dir())
                    )),
            )
    }

    fn secondary_label(&self, entry: &EntryRef) -> String {
        match entry.kind {
            Kind::Context => {
                let (cluster, user) = self.doc.context_refs(&entry.name);
                [cluster, user]
                    .into_iter()
                    .flatten()
                    .collect::<Vec<_>>()
                    .join(" · ")
            }
            Kind::Cluster => self
                .doc
                .body(Kind::Cluster, &entry.name)
                .map(|b| crate::model::get_str(b, &["server"]))
                .map(|s| {
                    s.trim_start_matches("https://")
                        .trim_start_matches("http://")
                        .to_string()
                })
                .unwrap_or_default(),
            Kind::User => self
                .doc
                .body(Kind::User, &entry.name)
                .map(|b| match AuthKind::detect(b) {
                    AuthKind::Exec => {
                        format!("exec · {}", crate::model::get_str(b, &["exec", "command"]))
                    }
                    other => other.label().to_lowercase(),
                })
                .unwrap_or_default(),
        }
    }

    fn render_test_panel(&self, cx: &mut Context<Self>) -> Option<AnyElement> {
        if !self.show_test {
            return None;
        }
        let context = self.selected_context()?;
        let colors = cx.colors().clone();
        let key = TestKey::new(self.doc_key(), context.clone());
        let global = Kubeconfigs::global(cx);
        let report = global.read(cx).report(&key).cloned();
        let sign_in = global.read(cx).sign_in_state(&key).cloned();
        let weak = cx.entity().downgrade();
        let content = match report {
            None => widgets::hint(format!("Test {context} to check it step by step."), &colors),
            Some(report) => {
                let mut handlers = panel::Handlers::default();
                {
                    let weak = weak.clone();
                    let context = context.clone();
                    handlers.retest = Some(Rc::new(move |window, cx| {
                        let context = context.clone();
                        weak.update(cx, |this, cx| this.test(context, window, cx))
                            .ok();
                    }));
                }
                {
                    let weak = weak.clone();
                    let context = context.clone();
                    handlers.fetch_ca = Some(Rc::new(move |window, cx| {
                        let Some(this) = weak.upgrade() else { return };
                        let cluster = this.read(cx).doc.context_refs(&context).0;
                        if let Some(cluster) = cluster {
                            crate::dialogs::fetch_ca_for(weak.clone(), cluster, window, cx);
                        }
                    }));
                }
                if let Some(oidc) = report.oidc.clone() {
                    let sign_in_key = key.clone();
                    handlers.sign_in = Some(Rc::new(move |_, cx| {
                        let key = sign_in_key.clone();
                        let oidc = oidc.clone();
                        Kubeconfigs::global(cx).update(cx, |g, cx| g.sign_in(key, oidc, cx));
                    }));
                    let key = key.clone();
                    handlers.cancel_sign_in = Some(Rc::new(move |_, cx| {
                        let key = key.clone();
                        Kubeconfigs::global(cx).update(cx, |g, cx| g.cancel_sign_in(&key, cx));
                    }));
                }
                {
                    let weak = weak.clone();
                    let context = context.clone();
                    handlers.edit_user = Some(Rc::new(move |window, cx| {
                        let context = context.clone();
                        weak.update(cx, |this, cx| {
                            if let Some(user) = this.doc.context_refs(&context).1 {
                                this.set_tab(Tab::Form, window, cx);
                                this.select(Some(EntryRef::new(Kind::User, user)), window, cx);
                            }
                        })
                        .ok();
                    }));
                }
                {
                    let weak = weak.clone();
                    let context = context.clone();
                    handlers.namespace = Some(Rc::new(move |ns, window, cx| {
                        let ns = ns.to_string();
                        let context = context.clone();
                        weak.update(cx, |this, cx| {
                            this.edit(window, cx, |doc| {
                                if let Some(body) = doc.body_mut(Kind::Context, &context) {
                                    crate::model::set_str(body, &["namespace"], &ns);
                                }
                            });
                            this.rebuild_form(window, cx);
                        })
                        .ok();
                    }));
                }
                let subtitle = if self.is_dirty() {
                    "from unsaved edits"
                } else {
                    "from the saved file"
                };
                panel::report(
                    "kc-report",
                    &report,
                    Some(subtitle.into()),
                    sign_in.as_ref(),
                    &handlers,
                    &colors,
                )
            }
        };
        Some(
            v_flex()
                .w(u(420.0))
                .flex_none()
                .h_full()
                .bg(colors.panel)
                .border_l_1()
                .border_color(colors.border)
                .child(h_flex().justify_end().px(u(8.0)).pt(u(6.0)).child(
                    kubyl_ui::IconButton::new("kc-close-test", IconName::Minus).on_click(
                        move |_, _, cx| {
                            weak.update(cx, |this, cx| {
                                this.show_test = false;
                                cx.notify();
                            })
                            .ok();
                        },
                    ),
                ))
                .child(
                    div()
                        .id("kc-test-scroll")
                        .flex_1()
                        .overflow_y_scroll()
                        .px(u(16.0))
                        .pb(u(16.0))
                        .child(content),
                )
                .into_any_element(),
        )
    }

    fn render_yaml(&self, cx: &mut Context<Self>) -> AnyElement {
        let colors = cx.colors().clone();
        let weak = cx.entity().downgrade();
        let revealed = self.yaml.revealed;
        v_flex()
            .flex_1()
            .min_w_0()
            .h_full()
            .child(
                h_flex()
                    .h(u(32.0))
                    .px(u(12.0))
                    .gap(u(8.0))
                    .border_b_1()
                    .border_color(colors.border_variant)
                    .child(
                        div()
                            .flex_1()
                            .text_size(u(11.5))
                            .text_color(if self.yaml.parse_error.is_some() {
                                colors.red
                            } else {
                                colors.text_dim
                            })
                            .child(match &self.yaml.parse_error {
                                Some(err) => format!(
                                    "Doesn't parse: {err}. The form keeps the last valid state."
                                ),
                                None => {
                                    "The file as it will be written. Edits here update the form."
                                        .into()
                                }
                            }),
                    )
                    .child(
                        Button::new("kc-reveal")
                            .ghost()
                            .icon(if revealed {
                                IconName::EyeOff
                            } else {
                                IconName::Eye
                            })
                            .label(if revealed {
                                "Mask secrets"
                            } else {
                                "Reveal secrets"
                            })
                            .on_click(move |_, window, cx| {
                                weak.update(cx, |this, cx| this.toggle_secrets(window, cx))
                                    .ok();
                            }),
                    ),
            )
            .child(
                div()
                    .flex_1()
                    .min_h_0()
                    .font_family(fonts::MONO)
                    .text_size(u(13.0))
                    .child(Editor::new(&self.yaml.editor).h_full().appearance(false)),
            )
            .into_any_element()
    }
}

impl Render for KubeconfigEditor {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let colors = cx.colors().clone();
        let root = v_flex()
            .key_context(CONTEXT)
            .track_focus(&self.focus)
            .size_full()
            .bg(colors.background)
            .text_color(colors.text)
            .font_family(fonts::UI)
            .text_size(u(sizes::UI_FONT))
            .on_action(cx.listener(|this, _: &Save, window, cx| {
                let weak = cx.entity().downgrade();
                if this.is_dirty() && !this.saving {
                    crate::dialogs::save_preview(weak, window, cx);
                }
            }))
            .on_action(cx.listener(|this, _: &TestConnection, window, cx| {
                if let Some(context) = this.selected_context() {
                    this.test(context, window, cx);
                }
            }))
            .on_action(cx.listener(|this, _: &TestAll, window, cx| this.test_all(window, cx)))
            .on_action(
                cx.listener(|this, _: &ShowForm, window, cx| this.set_tab(Tab::Form, window, cx)),
            )
            .on_action(
                cx.listener(|this, _: &ShowYaml, window, cx| this.set_tab(Tab::Yaml, window, cx)),
            )
            .on_action(cx.listener(|this, _: &Revert, window, cx| this.revert(window, cx)))
            .on_action(
                cx.listener(|this, _: &ToggleSecrets, window, cx| this.toggle_secrets(window, cx)),
            );
        if self.loading {
            return root.child(centered("Loading…", &colors));
        }
        if let Some(err) = &self.load_error {
            return root.child(centered(err, &colors));
        }
        let center = match self.tab {
            Tab::Form => div()
                .id("kc-form")
                .flex_1()
                .min_w_0()
                .h_full()
                .overflow_y_scroll()
                .p(u(18.0))
                .child(self.render_form(window, cx))
                .into_any_element(),
            Tab::Yaml => self.render_yaml(cx),
        };
        let hints = KeyHints::new([
            (
                SharedString::from(kubyl_ui::format_keystroke("secondary-s")),
                SharedString::from("Save…"),
            ),
            (
                kubyl_ui::format_keystroke("secondary-shift-t").into(),
                "Test connection".into(),
            ),
            (
                kubyl_ui::format_keystroke("secondary-enter").into(),
                "Test all".into(),
            ),
            (
                kubyl_ui::format_keystroke("secondary-1").into(),
                "Form".into(),
            ),
            (
                kubyl_ui::format_keystroke("secondary-2").into(),
                "YAML".into(),
            ),
            (
                kubyl_ui::format_keystroke("secondary-shift-r").into(),
                "Reveal secrets".into(),
            ),
        ]);
        root.child(self.render_toolbar(cx))
            .children(self.render_banner(cx))
            .child(
                h_flex()
                    .flex_1()
                    .min_h_0()
                    .child(self.render_list(cx))
                    .child(center)
                    .children(self.render_test_panel(cx)),
            )
            .child(hints)
    }
}

fn centered(text: &str, colors: &Colors) -> AnyElement {
    div()
        .flex_1()
        .flex()
        .items_center()
        .justify_center()
        .text_color(colors.text_dim)
        .child(text.to_string())
        .into_any_element()
}
