//! Columns for the explorer's tables:
//! - the generic tables of Flux kinds (Custom Resources → *.toolkit.fluxcd.io), with state,
//!   source and revision instead of only the CRDs' printer columns;
//! - a "Flux" column in Deployments, StatefulSets and DaemonSets: the managing Flux object with
//!   its state on the row's cluster, a link to it. It shows in tables of clusters that serve
//!   Flux (re-evaluated when their discovery changes).

use std::cell::RefCell;
use std::collections::HashMap;
use std::sync::Arc;

use gpui::App;
use kubyl_core::actions::OpenView;
use kubyl_core::{
    CellButton, CellValue, ClusterId, ColumnDef, ColumnProvider, ColumnWidth, ResourceColumns,
    Tone, ViewKind, ViewRequest,
};
use kubyl_flux_core::kinds::FluxKind;
use kubyl_flux_core::model::FluxObject;
use kubyl_flux_core::ownership;
use kubyl_flux_core::rows::Row;
use kubyl_ui::IconName;
use serde_json::Value;

use crate::state::FluxIndex;
use crate::widgets::state_tone;

fn text(value: impl Into<String>) -> CellValue {
    CellValue::Text(value.into().into())
}

fn muted(value: impl Into<String>) -> CellValue {
    CellValue::Tinted {
        label: value.into().into(),
        tone: Tone::Neutral,
    }
}

/// Rows cached per object version: a table renders several cells per row, and parsing needs
/// the object behind an `Arc` (cells get a reference), so each version is copied once.
const ROW_CACHE: usize = 4096;

/// The generic table of a Flux kind.
struct FluxColumns {
    kind: FluxKind,
    rows: RefCell<HashMap<(String, String), Arc<Row>>>,
}

impl FluxColumns {
    fn new(kind: FluxKind) -> Self {
        Self {
            kind,
            rows: RefCell::default(),
        }
    }

    fn row(&self, object: &Value) -> Option<Arc<Row>> {
        let text = |pointer: &str| object.pointer(pointer).and_then(Value::as_str);
        let key = text("/metadata/uid")
            .zip(text("/metadata/resourceVersion"))
            .map(|(uid, version)| (uid.to_string(), version.to_string()));
        if let Some(key) = &key
            && let Some(row) = self.rows.borrow().get(key)
        {
            return Some(row.clone());
        }
        let row = Arc::new(Row::new(FluxObject::parse_as(
            self.kind,
            &Arc::new(object.clone()),
        )?));
        if let Some(key) = key {
            let mut rows = self.rows.borrow_mut();
            if rows.len() >= ROW_CACHE {
                rows.clear();
            }
            rows.insert(key, row.clone());
        }
        Some(row)
    }
}

impl ColumnProvider for FluxColumns {
    fn columns(&self) -> Vec<ColumnDef> {
        let flex =
            |id, title, min| ColumnDef::new(id, title, ColumnWidth::Flex { weight: 1.0, min });
        vec![
            flex("name", "Name", 150.0).mono(),
            ColumnDef::new("state", "State", ColumnWidth::Fixed(110.0)),
            flex("message", "Message", 200.0),
            flex("source", "Source", 140.0).mono(),
            ColumnDef::new("revision", "Revision", ColumnWidth::Fixed(150.0)).mono(),
            ColumnDef::new("interval", "Interval", ColumnWidth::Fixed(64.0)).mono(),
            ColumnDef::new("age", "Age", ColumnWidth::Fixed(54.0)).mono(),
        ]
    }

    fn cell(&self, object: &Value, column: &str) -> CellValue {
        match column {
            "name" => {
                return text(
                    object
                        .pointer("/metadata/name")
                        .and_then(Value::as_str)
                        .unwrap_or_default(),
                );
            }
            "age" => {
                return muted(kubyl_resources::format::object_age(
                    object,
                    jiff::Timestamp::now(),
                ));
            }
            _ => {}
        }
        let Some(row) = self.row(object) else {
            return CellValue::Empty;
        };
        match column {
            "state" => CellValue::Status {
                label: row.state.label().into(),
                tone: state_tone(row.state),
            },
            "message" => muted(row.message.clone()),
            "source" => text(row.source.clone()),
            "revision" => text(row.revision.clone()),
            "interval" => muted(row.object.interval.clone().unwrap_or_default()),
            _ => CellValue::Empty,
        }
    }
}

/// The "Flux" column of workload lists.
struct ManagedColumn(Arc<FluxIndex>);

impl ManagedColumn {
    fn column() -> Vec<ColumnDef> {
        vec![ColumnDef::new("flux", "Flux", ColumnWidth::Fixed(150.0))]
    }

    /// The cell of `object` on `cluster` (`None`: the cluster isn't known, no state).
    fn managed_cell(&self, cluster: Option<&ClusterId>, object: &Value) -> CellValue {
        let Some(manager) = ownership::managed_by(object) else {
            return CellValue::Empty;
        };
        let Some(kind) = manager.kind else {
            return CellValue::Empty;
        };
        let state = cluster.and_then(|c| self.0.state(c, kind, &manager.namespace, &manager.name));
        let label = format!("{}/{}", kind.short(), manager.name);
        let tooltip = format!(
            "Managed by Flux {} {} · {}",
            kind,
            manager.key(),
            state.map_or("not loaded", |s| s.label())
        );
        let namespace = manager.namespace.clone();
        let name = manager.name.clone();
        CellValue::Buttons(vec![
            CellButton::new(label, move |row| {
                let gvr = kubyl_core::Gvr::new(
                    kind.group(),
                    crate::state::ga_version(kind),
                    kind.plural(),
                );
                Box::new(OpenView(ViewRequest::for_resource(
                    ViewKind::Custom(crate::views::object::VIEW_KIND.into()),
                    kubyl_core::ResourceRef::object(
                        row.cluster.clone(),
                        gvr,
                        Some(namespace.clone()),
                        name.clone(),
                    ),
                )))
            })
            .icon(
                match state {
                    Some(s) if s.is_problem() => IconName::CircleX,
                    Some(kubyl_flux_core::model::State::Suspended) => IconName::Pause,
                    Some(kubyl_flux_core::model::State::Ready) => IconName::CircleCheck,
                    _ => crate::widgets::kind_icon(kind),
                }
                .path(),
            )
            .tooltip(tooltip)
            .active(state.is_some_and(|s| s.is_problem())),
        ])
    }
}

impl ColumnProvider for ManagedColumn {
    /// Without knowing the table's clusters: while any cluster serves Flux.
    fn columns(&self) -> Vec<ColumnDef> {
        if self.0.any() {
            Self::column()
        } else {
            Vec::new()
        }
    }

    fn cell(&self, object: &Value, column: &str) -> CellValue {
        if column != "flux" {
            return CellValue::Empty;
        }
        self.managed_cell(None, object)
    }

    /// Only in tables of clusters that serve Flux. Cheap: tables ask for every cell.
    fn columns_in(&self, clusters: &[ClusterId], _: &App) -> Vec<ColumnDef> {
        if clusters.iter().any(|c| self.0.serves(c)) {
            Self::column()
        } else {
            Vec::new()
        }
    }

    /// The managing object's state on the row's cluster.
    fn cell_in(&self, cluster: &ClusterId, object: &Value, column: &str, _: &App) -> CellValue {
        if column != "flux" {
            return CellValue::Empty;
        }
        self.managed_cell(Some(cluster), object)
    }
}

pub(crate) fn init(index: Arc<FluxIndex>, cx: &mut App) {
    for kind in FluxKind::ALL {
        ResourceColumns::register(cx, kind.group(), kind.kind(), FluxColumns::new(kind));
    }
    for kind in ["Deployment", "StatefulSet", "DaemonSet"] {
        ResourceColumns::extend(cx, "apps", kind, ManagedColumn(index.clone()));
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use kubyl_flux_core::fixtures;

    #[test]
    fn flux_kind_cells() {
        let columns = FluxColumns::new(FluxKind::Kustomization);
        assert_eq!(
            columns.cell(&fixtures::kustomization_failed(), "state"),
            CellValue::Status {
                label: "Failed".into(),
                tone: Tone::Bad
            }
        );
        assert_eq!(
            columns.cell(&fixtures::kustomization_ready(), "revision"),
            CellValue::Text("master@sha1:3e0ff8a".into())
        );
    }

    #[test]
    fn the_flux_column_follows_flux_and_names_the_manager() {
        let index = Arc::new(FluxIndex::default());
        let column = ManagedColumn(index.clone());
        // No cluster serves Flux: no column.
        assert!(column.columns().is_empty());
        match column.cell(&fixtures::managed_deployment(), "flux") {
            CellValue::Buttons(buttons) => assert_eq!(buttons[0].label.as_ref(), "ks/podinfo"),
            other => panic!("{other:?}"),
        }
        assert_eq!(
            column.cell(&serde_json::json!({"metadata": {"name": "x"}}), "flux"),
            CellValue::Empty
        );
    }

    /// Two clusters with the same Kustomization: each row shows its own cluster's state.
    #[gpui::test]
    fn the_flux_column_uses_the_rows_cluster(cx: &mut gpui::TestAppContext) {
        let index = Arc::new(FluxIndex::default());
        let ready = Arc::new(fixtures::kustomization_ready());
        let mut failed = fixtures::kustomization_ready();
        failed["status"]["conditions"][0]["status"] = "False".into();
        let failed = Arc::new(failed);
        let (a, b) = (ClusterId::new("a"), ClusterId::new("b"));
        index.replace(&a, [(FluxKind::Kustomization, &ready)]);
        index.replace(&b, [(FluxKind::Kustomization, &failed)]);
        let column = ManagedColumn(index.clone());
        let deployment = fixtures::managed_deployment();
        cx.update(|cx| {
            let button =
                |cluster: &ClusterId| match column.cell_in(cluster, &deployment, "flux", cx) {
                    CellValue::Buttons(buttons) => buttons[0].clone(),
                    other => panic!("{other:?}"),
                };
            assert!(!button(&a).active);
            assert!(button(&a).tooltip.unwrap().ends_with("Ready"));
            assert!(button(&b).active);
            assert!(button(&b).tooltip.unwrap().ends_with("Failed"));
            assert!(
                button(&ClusterId::new("c"))
                    .tooltip
                    .unwrap()
                    .ends_with("not loaded")
            );
            // A cluster without Flux: no column in its tables.
            assert!(column.columns_in(std::slice::from_ref(&a), cx).is_empty());
            index.set_serving(&a, true);
            assert_eq!(column.columns_in(&[b.clone(), a.clone()], cx).len(), 1);
            assert!(column.columns_in(std::slice::from_ref(&b), cx).is_empty());
            index.set_serving(&a, false);
            assert!(!index.any());
        });
        // Unchanged objects keep their parsed state; a cluster that goes is forgotten.
        index.replace(&a, [(FluxKind::Kustomization, &ready)]);
        assert_eq!(
            index.state(&a, FluxKind::Kustomization, "flux-demo", "podinfo"),
            Some(kubyl_flux_core::model::State::Ready)
        );
        index.forget(&a);
        assert_eq!(
            index.state(&a, FluxKind::Kustomization, "flux-demo", "podinfo"),
            None
        );
    }
}
