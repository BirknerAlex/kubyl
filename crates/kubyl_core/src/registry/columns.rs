use std::collections::HashMap;
use std::fmt;
use std::sync::Arc;

use gpui::{Action, App, Global, SharedString};

use crate::types::{Gvk, ResourceRef};

pub use kubyl_base::columns::{Align, ColumnDef, ColumnWidth};
pub use kubyl_base::types::Tone;

/// A rendered cell value, with the buttons the UI puts in cells.
pub type CellValue = kubyl_base::columns::CellValue<CellButton>;

/// Builds the action a [`CellButton`] dispatches, for the object of its row.
pub type CellAction = Arc<dyn Fn(&ResourceRef) -> Box<dyn Action>>;

/// A button inside a table cell.
#[derive(Clone)]
pub struct CellButton {
    pub label: SharedString,
    /// Asset path of an icon shown before the label.
    pub icon: Option<SharedString>,
    pub tooltip: Option<SharedString>,
    /// Highlighted (e.g. already open).
    pub active: bool,
    pub action: CellAction,
}

impl CellButton {
    pub fn new(
        label: impl Into<SharedString>,
        action: impl Fn(&ResourceRef) -> Box<dyn Action> + 'static,
    ) -> Self {
        Self {
            label: label.into(),
            icon: None,
            tooltip: None,
            active: false,
            action: Arc::new(action),
        }
    }

    pub fn icon(mut self, icon: impl Into<SharedString>) -> Self {
        self.icon = Some(icon.into());
        self
    }

    pub fn tooltip(mut self, tooltip: impl Into<SharedString>) -> Self {
        self.tooltip = Some(tooltip.into());
        self
    }

    pub fn active(mut self, active: bool) -> Self {
        self.active = active;
        self
    }
}

impl PartialEq for CellButton {
    fn eq(&self, other: &Self) -> bool {
        self.label == other.label
            && self.icon == other.icon
            && self.tooltip == other.tooltip
            && self.active == other.active
            && Arc::ptr_eq(&self.action, &other.action)
    }
}

impl fmt::Debug for CellButton {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("CellButton")
            .field("label", &self.label)
            .field("active", &self.active)
            .finish_non_exhaustive()
    }
}

/// Columns and cell extraction for one resource kind.
///
/// `object` is the resource as JSON (a kube `DynamicObject` serializes to this shape).
pub trait ColumnProvider: 'static {
    fn columns(&self) -> Vec<ColumnDef>;
    fn cell(&self, object: &serde_json::Value, column: &str) -> CellValue;
}

/// Per-kind column providers, keyed by group and kind (all versions share columns).
///
/// Kinds without a provider fall back to the server-side `Table` representation
/// (`Accept: application/json;as=Table;g=meta.k8s.io;v=v1`), which carries CRD printer
/// columns. That fallback lives in `kubyl_resources`.
#[derive(Default)]
pub struct ResourceColumns {
    providers: HashMap<(String, String), Arc<dyn ColumnProvider>>,
    /// Columns other crates add to a kind (web views on Services…), after its own.
    extensions: HashMap<(String, String), Vec<Arc<dyn ColumnProvider>>>,
    /// Providers with their extensions, rebuilt on every registration.
    merged: HashMap<(String, String), Arc<dyn ColumnProvider>>,
}

impl Global for ResourceColumns {}

impl ResourceColumns {
    pub fn register(cx: &mut App, group: &str, kind: &str, provider: impl ColumnProvider) {
        let this = cx.default_global::<Self>();
        let key = (group.to_string(), kind.to_string());
        this.providers.insert(key.clone(), Arc::new(provider));
        this.merge(key);
    }

    /// Adds columns to a kind that has a provider (in any order: registration of the kind's
    /// own provider may come later). Kinds shown through the server-side table don't get them.
    pub fn extend(cx: &mut App, group: &str, kind: &str, provider: impl ColumnProvider) {
        let this = cx.default_global::<Self>();
        let key = (group.to_string(), kind.to_string());
        this.extensions
            .entry(key.clone())
            .or_default()
            .push(Arc::new(provider));
        this.merge(key);
    }

    fn merge(&mut self, key: (String, String)) {
        let Some(base) = self.providers.get(&key).cloned() else {
            return;
        };
        let merged: Arc<dyn ColumnProvider> = match self.extensions.get(&key) {
            // Like the providers themselves: only used on the UI thread.
            #[allow(clippy::arc_with_non_send_sync)]
            Some(extensions) if !extensions.is_empty() => Arc::new(Extended {
                base,
                extensions: extensions.clone(),
            }),
            _ => base,
        };
        self.merged.insert(key, merged);
    }

    /// The provider for `gvk`, or `None` to use the server-side table.
    pub fn get(cx: &App, gvk: &Gvk) -> Option<Arc<dyn ColumnProvider>> {
        cx.try_global::<Self>()?
            .merged
            .get(&(gvk.group.clone(), gvk.kind.clone()))
            .cloned()
    }
}

/// A kind's provider followed by the columns other crates added.
struct Extended {
    base: Arc<dyn ColumnProvider>,
    extensions: Vec<Arc<dyn ColumnProvider>>,
}

impl ColumnProvider for Extended {
    /// The kind's columns with the added ones before `age` (which stays at the end of the
    /// regular columns), or last.
    fn columns(&self) -> Vec<ColumnDef> {
        let mut columns = self.base.columns();
        let at = columns
            .iter()
            .position(|c| c.id.as_ref() == "age")
            .unwrap_or(columns.len());
        let added: Vec<ColumnDef> = self
            .extensions
            .iter()
            .flat_map(|extension| extension.columns())
            .collect();
        columns.splice(at..at, added);
        columns
    }

    fn cell(&self, object: &serde_json::Value, column: &str) -> CellValue {
        for extension in &self.extensions {
            if extension.columns().iter().any(|c| c.id.as_ref() == column) {
                return extension.cell(object, column);
            }
        }
        self.base.cell(object, column)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    struct Fixed(&'static str, &'static str);

    impl ColumnProvider for Fixed {
        fn columns(&self) -> Vec<ColumnDef> {
            vec![ColumnDef::new(self.0, self.0, ColumnWidth::Fixed(80.0))]
        }

        fn cell(&self, _: &serde_json::Value, _: &str) -> CellValue {
            CellValue::Text(self.1.into())
        }
    }

    struct AgeThenWide;

    impl ColumnProvider for AgeThenWide {
        fn columns(&self) -> Vec<ColumnDef> {
            ["name", "age", "selector"]
                .into_iter()
                .map(|id| ColumnDef::new(id, id, ColumnWidth::Fixed(80.0)))
                .collect()
        }

        fn cell(&self, _: &serde_json::Value, _: &str) -> CellValue {
            CellValue::Empty
        }
    }

    #[gpui::test]
    fn extensions_add_columns_in_any_order(cx: &mut gpui::TestAppContext) {
        cx.update(|cx| {
            ResourceColumns::extend(cx, "", "Service", Fixed("web", "80"));
            ResourceColumns::register(cx, "", "Service", Fixed("name", "grafana"));
            let provider = ResourceColumns::get(cx, &Gvk::new("", "v1", "Service")).unwrap();
            let ids: Vec<_> = provider.columns().iter().map(|c| c.id.clone()).collect();
            assert_eq!(ids, ["name", "web"]);
            let object = serde_json::json!({});
            assert_eq!(provider.cell(&object, "web"), CellValue::Text("80".into()));
            assert_eq!(
                provider.cell(&object, "name"),
                CellValue::Text("grafana".into())
            );
            // Added columns go before age (wide-only columns may follow it).
            ResourceColumns::register(cx, "", "Service", AgeThenWide);
            let provider = ResourceColumns::get(cx, &Gvk::new("", "v1", "Service")).unwrap();
            let ids: Vec<_> = provider.columns().iter().map(|c| c.id.clone()).collect();
            assert_eq!(ids, ["name", "web", "age", "selector"]);
            // No provider of its own: extensions alone don't create one.
            ResourceColumns::extend(cx, "", "Pod", Fixed("web", "80"));
            assert!(ResourceColumns::get(cx, &Gvk::new("", "v1", "Pod")).is_none());
        });
    }
}
