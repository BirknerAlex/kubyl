//! Hand-written column sets for the core kinds, matching `kubectl get` (and `-o wide`): the
//! columns and cells are `kubyl_resources_core::columns`; this registers them for the views.
//!
//! Kinds without a provider use the server-side `Table` ([`crate::table`]), which carries CRD
//! printer columns. CPU and memory cells are [`CellValue::Empty`] here: list views fill them from
//! [`crate::metrics`] (phase 07 provides the data).

use std::convert::Infallible;

use gpui::App;
use kubyl_core::{CellValue, ColumnDef, ColumnProvider, ResourceColumns};
pub use kubyl_resources_core::columns::*;
use serde_json::Value;

/// A built-in kind's columns for the views.
struct Provider(Kind);

impl ColumnProvider for Provider {
    fn columns(&self) -> Vec<ColumnDef> {
        self.0.columns()
    }

    fn cell(&self, object: &Value, column: &str) -> CellValue {
        self.0
            .cell(object, column)
            .map_buttons(|button: Infallible| match button {})
    }
}

/// Registers the built-in column sets.
pub fn register(cx: &mut App) {
    for (group, kind, columns) in builtin() {
        ResourceColumns::register(cx, group, kind, Provider(columns));
    }
}
