//! What the user selected in a resource list, for actions registered by any crate.
//!
//! List views update [`ResourceSelection`] when their selection or focus changes. An action
//! handler (`cx.on_action(|_: &ShowLogs, cx| …)`) reads it to know its targets:
//!
//! ```ignore
//! let selection = ResourceSelection::global(cx);
//! let Some(pod) = selection.primary() else { return };
//! ```

use std::sync::Arc;

use gpui::{App, Entity, Global};
use kubyl_core::{ClusterCaps, ResourceRef};
use serde_json::Value;

use crate::store::ResourceStore;

/// One selected object.
#[derive(Clone)]
pub struct Selected {
    pub target: ResourceRef,
    /// `Pod`, `Deployment`…
    pub kind: String,
    /// The object when the list has it (full or metadata-only), for instant details.
    pub object: Option<Arc<Value>>,
    /// The store the object lives in, to follow live updates.
    pub store: Option<Entity<ResourceStore>>,
}

/// The current selection of the focused resource list (or details view).
#[derive(Clone, Default)]
pub struct ResourceSelection {
    /// Selected objects; the first is the primary one (the details dock shows it).
    pub items: Vec<Selected>,
    /// Capabilities of the selection's cluster.
    pub caps: ClusterCaps,
}

impl Global for ResourceSelection {}

impl ResourceSelection {
    pub fn global(cx: &App) -> &Self {
        cx.global::<Self>()
    }

    /// Replaces the selection. Observers (`cx.observe_global::<ResourceSelection>`) update.
    pub fn set(cx: &mut App, selection: ResourceSelection) {
        cx.set_global(selection);
    }

    pub fn primary(&self) -> Option<&Selected> {
        self.items.first()
    }

    pub fn targets(&self) -> impl Iterator<Item = &ResourceRef> {
        self.items.iter().map(|s| &s.target)
    }

    pub fn is_empty(&self) -> bool {
        self.items.is_empty()
    }
}

pub(crate) fn init(cx: &mut App) {
    cx.default_global::<ResourceSelection>();
}
