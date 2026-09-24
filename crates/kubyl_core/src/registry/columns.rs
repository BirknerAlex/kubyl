use std::collections::HashMap;
use std::sync::Arc;

use gpui::{App, Global, SharedString};

use crate::types::Gvk;

/// How wide a column is.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum ColumnWidth {
    /// Fixed width in (unscaled) pixels.
    Fixed(f32),
    /// Share of the remaining width, like CSS `fr`, with a minimum in pixels.
    Flex { weight: f32, min: f32 },
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum Align {
    #[default]
    Start,
    End,
}

/// Semantic color of a status value; the UI maps it to theme colors.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum Tone {
    #[default]
    Neutral,
    /// Running, Ready, Succeeded.
    Good,
    /// Pending, Upgrade available.
    Warning,
    /// CrashLoopBackOff, Failed, NotReady.
    Bad,
    /// ContainerCreating, Installing.
    Info,
    /// Completed, disabled.
    Muted,
}

/// One table column.
#[derive(Clone, Debug, PartialEq)]
pub struct ColumnDef {
    pub id: SharedString,
    /// Header label; rendered upper-case.
    pub title: SharedString,
    pub width: ColumnWidth,
    pub align: Align,
    /// Render cells in the monospace font (names, numbers, ages).
    pub mono: bool,
    /// Only shown in the "wide" view (like `kubectl get -o wide`).
    pub wide: bool,
}

impl ColumnDef {
    pub fn new(
        id: impl Into<SharedString>,
        title: impl Into<SharedString>,
        width: ColumnWidth,
    ) -> Self {
        Self {
            id: id.into(),
            title: title.into(),
            width,
            align: Align::Start,
            mono: false,
            wide: false,
        }
    }

    pub fn mono(mut self) -> Self {
        self.mono = true;
        self
    }

    pub fn align_end(mut self) -> Self {
        self.align = Align::End;
        self
    }

    /// Hidden unless the view is in "wide" mode.
    pub fn wide(mut self) -> Self {
        self.wide = true;
        self
    }
}

/// A rendered cell value.
#[derive(Clone, Debug, PartialEq)]
pub enum CellValue {
    Text(SharedString),
    /// Text in a status color without a dot, e.g. a restart count in red.
    Tinted {
        label: SharedString,
        tone: Tone,
    },
    Status {
        label: SharedString,
        tone: Tone,
    },
    /// A value with a usage bar, e.g. CPU `184m` at 42%.
    Usage {
        label: SharedString,
        percent: f32,
    },
    /// Renders as a faint dash.
    Empty,
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
}

impl Global for ResourceColumns {}

impl ResourceColumns {
    pub fn register(cx: &mut App, group: &str, kind: &str, provider: impl ColumnProvider) {
        cx.default_global::<Self>()
            .providers
            .insert((group.to_string(), kind.to_string()), Arc::new(provider));
    }

    /// The provider for `gvk`, or `None` to use the server-side table.
    pub fn get(cx: &App, gvk: &Gvk) -> Option<Arc<dyn ColumnProvider>> {
        cx.try_global::<Self>()?
            .providers
            .get(&(gvk.group.clone(), gvk.kind.clone()))
            .cloned()
    }
}
