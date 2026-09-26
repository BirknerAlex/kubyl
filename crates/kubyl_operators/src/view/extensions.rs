//! OLM v1: ClusterExtensions and ClusterCatalogs, read-only, with a YAML template to install one.

use gpui::{AnyElement, Context, FontWeight, IntoElement, Window, div, prelude::*};
use kubyl_core::actions::OpenView;
use kubyl_core::{ColumnDef, ColumnWidth, ResourceRef, Tone, ViewKind, ViewRequest};
use kubyl_ui::{ActiveColors, Button, IconName, fonts, h_flex, u, v_flex};

use super::{OperatorsView, SubTab};
use crate::olm::v1::{ClusterCatalog, ClusterExtension};
use crate::widgets;

fn extension_key(ext: &ClusterExtension) -> String {
    format!("ext:{}", ext.name)
}

impl OperatorsView {
    pub(crate) fn render_extensions(
        &mut self,
        _: &mut Window,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let colors = cx.colors().clone();
        let Some(snapshot) = self.snapshot(cx) else {
            return widgets::empty("Loading…", &colors);
        };
        let query = self.query(cx);
        let extensions: Vec<ClusterExtension> = snapshot
            .extensions
            .iter()
            .filter(|e| {
                query.is_empty()
                    || Self::matches(
                        &query,
                        &format!(
                            "{} {} {}",
                            e.name,
                            e.package.as_deref().unwrap_or_default(),
                            e.namespace.as_deref().unwrap_or_default()
                        ),
                    )
            })
            .cloned()
            .collect();
        self.keys = extensions.iter().map(extension_key).collect();
        if self.selected_key().is_none()
            && let Some(first) = self.keys.first().cloned()
        {
            self.selected.insert(SubTab::Extensions, first);
        }
        let selected = self.selected_key().cloned();
        let read_only = self.read_only(cx);
        let columns = vec![
            ColumnDef::new(
                "name",
                "ClusterExtension",
                ColumnWidth::Flex {
                    weight: 1.0,
                    min: 140.0,
                },
            ),
            ColumnDef::new(
                "package",
                "Package",
                ColumnWidth::Flex {
                    weight: 1.0,
                    min: 140.0,
                },
            ),
            ColumnDef::new("version", "Version", ColumnWidth::Fixed(90.0)),
            ColumnDef::new("namespace", "Namespace", ColumnWidth::Fixed(130.0)),
            ColumnDef::new("status", "Status", ColumnWidth::Fixed(130.0)),
        ];
        let catalog_columns = vec![
            ColumnDef::new(
                "name",
                "ClusterCatalog",
                ColumnWidth::Flex {
                    weight: 1.0,
                    min: 140.0,
                },
            ),
            ColumnDef::new(
                "image",
                "Image",
                ColumnWidth::Flex {
                    weight: 1.5,
                    min: 160.0,
                },
            ),
            ColumnDef::new("poll", "Poll", ColumnWidth::Fixed(100.0)),
            ColumnDef::new("status", "Status", ColumnWidth::Fixed(130.0)),
        ];
        let rows: Vec<AnyElement> = extensions
            .iter()
            .enumerate()
            .map(|(i, ext)| {
                let key = extension_key(ext);
                let (status, tone) = ext.status();
                let cells: Vec<AnyElement> = vec![
                    widgets::mono(ext.name.clone()),
                    widgets::mono(ext.package.clone().unwrap_or_default()),
                    widgets::mono(ext.version.clone().unwrap_or_else(|| "—".into())),
                    widgets::mono(ext.namespace.clone().unwrap_or_default()),
                    widgets::pill(status, tone, &colors),
                ];
                widgets::row(
                    ("ext-row", i),
                    selected.as_ref() == Some(&key),
                    32.0,
                    &colors,
                )
                .children(
                    columns
                        .iter()
                        .zip(cells)
                        .map(|(def, cell)| widgets::column_cell(def).child(cell)),
                )
                .on_click(cx.listener(move |this, _, window, cx| {
                    this.focus.focus(window, cx);
                    this.select(key.clone(), cx);
                }))
                .into_any_element()
            })
            .collect();
        let catalog_rows: Vec<AnyElement> = snapshot
            .cluster_catalogs
            .iter()
            .enumerate()
            .map(|(i, catalog): (usize, &ClusterCatalog)| {
                let cells: Vec<AnyElement> = vec![
                    widgets::mono(catalog.name.clone()),
                    widgets::mono(catalog.image.clone().unwrap_or_default()),
                    widgets::text(
                        catalog
                            .poll_minutes
                            .map(|m| format!("every {m}m"))
                            .unwrap_or_else(|| "—".into()),
                    ),
                    if catalog.serving() {
                        widgets::pill("Serving", Tone::Good, &colors)
                    } else {
                        widgets::pill("Not serving", Tone::Warning, &colors)
                    },
                ];
                widgets::row(("catalog-row", i), false, 32.0, &colors)
                    .children(
                        catalog_columns
                            .iter()
                            .zip(cells)
                            .map(|(def, cell)| widgets::column_cell(def).child(cell)),
                    )
                    .into_any_element()
            })
            .collect();
        let selected_ext =
            selected.and_then(|k| extensions.iter().find(|e| extension_key(e) == k).cloned());
        let cluster = self.cluster.clone();
        let details = selected_ext.map(|ext| {
            let yaml_ref = ResourceRef::object(
                cluster.clone(),
                crate::olm::v1::cluster_extensions(),
                None,
                ext.name.clone(),
            );
            let mut section = widgets::section(format!("{} · conditions", ext.name), &colors);
            for condition in &ext.conditions {
                let good = condition.is_true() && condition.kind != "Deprecated";
                section = section.child(widgets::note(
                    if good {
                        IconName::CircleCheck
                    } else {
                        IconName::TriangleAlert
                    },
                    if good { colors.green } else { colors.yellow },
                    format!(
                        "{} · {}{}",
                        condition.kind,
                        condition.status,
                        condition
                            .message
                            .as_ref()
                            .map(|m| format!(" · {m}"))
                            .unwrap_or_default()
                    ),
                    &colors,
                ));
            }
            section
                .child(
                    div()
                        .text_size(u(11.5))
                        .text_color(colors.text_dim)
                        .child(format!(
                            "{}Upgrade by editing spec.source.catalog.version (Edit YAML, e).",
                            ext.bundle
                                .as_ref()
                                .map(|b| format!("Installed bundle {b}. "))
                                .unwrap_or_default()
                        )),
                )
                .child(
                    h_flex().gap(u(6.0)).child(
                        Button::new("ext-yaml")
                            .ghost()
                            .icon(IconName::Code)
                            .label("Edit YAML")
                            .on_click(move |_, window, cx| {
                                window.dispatch_action(
                                    Box::new(OpenView(ViewRequest::for_resource(
                                        ViewKind::Yaml,
                                        yaml_ref.clone(),
                                    ))),
                                    cx,
                                )
                            }),
                    ),
                )
        });
        self.focus_area()
            .child(
                v_flex()
                    .size_full()
                    .child(
                        h_flex()
                            .flex_none()
                            .px(u(14.0))
                            .py(u(8.0))
                            .gap(u(8.0))
                            .border_b_1()
                            .border_color(colors.border_variant)
                            .text_size(u(12.5))
                            .text_color(colors.text_muted)
                            .child(kubyl_ui::Icon::new(IconName::Info).size(13.0).color(colors.accent))
                            .child(div().flex_1().child("OLM v1: Kubyl lists extensions and catalogs. Install or upgrade one from a YAML template."))
                            .when(!read_only, |this| {
                                this.child(
                                    Button::new("new-extension")
                                        .icon(IconName::FilePlus)
                                        .label("New ClusterExtension…")
                                        .on_click(cx.listener(|this, _, window, cx| this.new_extension(window, cx))),
                                )
                            }),
                    )
                    .child(widgets::header(&columns, &colors))
                    .children(if rows.is_empty() {
                        vec![div()
                            .px(u(14.0))
                            .py(u(10.0))
                            .text_size(u(12.5))
                            .text_color(colors.text_dim)
                            .child("No ClusterExtensions.")
                            .into_any_element()]
                    } else {
                        rows
                    })
                    .child(div().h(u(14.0)))
                    .child(widgets::header(&catalog_columns, &colors))
                    .children(catalog_rows)
                    .child(div().flex_1())
                    .children(details.map(|d| {
                        div()
                            .border_t_1()
                            .border_color(colors.border_variant)
                            .child(d)
                            .into_any_element()
                    }))
                    .child(
                        div()
                            .px(u(14.0))
                            .py(u(6.0))
                            .text_size(u(11.0))
                            .font_family(fonts::MONO)
                            .text_color(colors.text_faint)
                            .font_weight(FontWeight::NORMAL)
                            .child("olm.operatorframework.io/v1"),
                    ),
            )
            .into_any_element()
    }
}
