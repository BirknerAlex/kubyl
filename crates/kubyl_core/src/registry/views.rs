use std::collections::HashMap;
use std::sync::Arc;

use gpui::{
    AnyView, App, AppContext as _, Entity, EntityId, FocusHandle, Focusable, Global, Hsla, Render,
    SharedString, Subscription, Window,
};
use serde::{Deserialize, Serialize};

use crate::types::{ResourceRef, ViewKind};

/// What to open: a view kind, optionally for a resource. Persisted to restore tabs.
#[derive(Clone, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct ViewRequest {
    pub kind: ViewKind,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub target: Option<ResourceRef>,
}

impl ViewRequest {
    pub fn new(kind: ViewKind) -> Self {
        Self { kind, target: None }
    }

    pub fn for_resource(kind: ViewKind, target: ResourceRef) -> Self {
        Self {
            kind,
            target: Some(target),
        }
    }
}

/// A view that can live in a tab (center pane or dock).
pub trait TabView: Render + Focusable {
    /// Tab label.
    fn tab_title(&self, cx: &App) -> SharedString;

    /// Asset path of the tab icon, e.g. `kubyl_ui::IconName::Box.path()`.
    fn tab_icon(&self, _cx: &App) -> Option<SharedString> {
        None
    }

    /// Unsaved changes: the tab shows a dot instead of the close button.
    fn is_dirty(&self, _cx: &App) -> bool {
        false
    }

    /// A colored dot before the icon, e.g. the color of the view's cluster.
    fn tab_dot(&self, _cx: &App) -> Option<Hsla> {
        None
    }

    /// The view asks its pane to close it (its session was stopped elsewhere). Checked
    /// whenever the view notifies.
    fn wants_close(&self, _cx: &App) -> bool {
        false
    }

    /// The request that rebuilds this view on the next start. `None` = not restored.
    fn view_request(&self, _cx: &App) -> Option<ViewRequest> {
        None
    }
}

/// Type-erased handle to a [`TabView`] entity. Panes and docks hold these.
pub trait TabHandle: 'static {
    fn entity_id(&self) -> EntityId;
    fn title(&self, cx: &App) -> SharedString;
    fn icon(&self, cx: &App) -> Option<SharedString>;
    fn is_dirty(&self, cx: &App) -> bool;
    fn dot(&self, cx: &App) -> Option<Hsla>;
    fn wants_close(&self, cx: &App) -> bool;
    fn view_request(&self, cx: &App) -> Option<ViewRequest>;
    fn focus_handle(&self, cx: &App) -> FocusHandle;
    fn to_any_view(&self) -> AnyView;
    fn boxed_clone(&self) -> Box<dyn TabHandle>;
    /// Calls `on_change` whenever the view notifies (title, icon or dirty state may have changed).
    fn observe(&self, cx: &mut App, on_change: Box<dyn Fn(&mut App)>) -> Subscription;
}

impl<T: TabView> TabHandle for Entity<T> {
    fn entity_id(&self) -> EntityId {
        Entity::entity_id(self)
    }

    fn title(&self, cx: &App) -> SharedString {
        self.read(cx).tab_title(cx)
    }

    fn icon(&self, cx: &App) -> Option<SharedString> {
        self.read(cx).tab_icon(cx)
    }

    fn is_dirty(&self, cx: &App) -> bool {
        self.read(cx).is_dirty(cx)
    }

    fn dot(&self, cx: &App) -> Option<Hsla> {
        self.read(cx).tab_dot(cx)
    }

    fn wants_close(&self, cx: &App) -> bool {
        self.read(cx).wants_close(cx)
    }

    fn view_request(&self, cx: &App) -> Option<ViewRequest> {
        self.read(cx).view_request(cx)
    }

    fn focus_handle(&self, cx: &App) -> FocusHandle {
        self.read(cx).focus_handle(cx)
    }

    fn to_any_view(&self) -> AnyView {
        self.clone().into()
    }

    fn boxed_clone(&self) -> Box<dyn TabHandle> {
        Box::new(self.clone())
    }

    fn observe(&self, cx: &mut App, on_change: Box<dyn Fn(&mut App)>) -> Subscription {
        cx.observe(self, move |_, cx| on_change(cx))
    }
}

/// Builds a view for a request. Returns `None` when the request can't be served (for example a
/// missing target).
pub type ViewFactory =
    Arc<dyn Fn(&ViewRequest, &mut Window, &mut App) -> Option<Box<dyn TabHandle>>>;

/// Maps each [`ViewKind`] to the crate that builds it.
///
/// ```ignore
/// ViewRegistry::register(cx, ViewKind::Logs, |request, window, cx| {
///     let target = request.target.clone()?;
///     Some(Box::new(cx.new(|cx| LogsView::new(target, window, cx))))
/// });
/// ```
#[derive(Default)]
pub struct ViewRegistry {
    factories: HashMap<ViewKind, ViewFactory>,
}

impl Global for ViewRegistry {}

impl ViewRegistry {
    /// Registers the factory for `kind`. A later registration replaces an earlier one.
    pub fn register(
        cx: &mut App,
        kind: ViewKind,
        factory: impl Fn(&ViewRequest, &mut Window, &mut App) -> Option<Box<dyn TabHandle>> + 'static,
    ) {
        let registry = cx.default_global::<Self>();
        if registry
            .factories
            .insert(kind.clone(), Arc::new(factory))
            .is_some()
        {
            tracing::warn!(kind = kind.as_str(), "view factory replaced");
        }
    }

    pub fn is_registered(cx: &App, kind: &ViewKind) -> bool {
        cx.try_global::<Self>()
            .is_some_and(|r| r.factories.contains_key(kind))
    }

    /// Builds a view for `request`, or `None` if no crate handles its kind.
    pub fn build(
        request: &ViewRequest,
        window: &mut Window,
        cx: &mut App,
    ) -> Option<Box<dyn TabHandle>> {
        let factory = cx
            .try_global::<Self>()?
            .factories
            .get(&request.kind)?
            .clone();
        factory(request, window, cx)
    }
}

/// Convenience for factories: wraps a new entity in a boxed [`TabHandle`].
pub fn new_tab<T: TabView>(
    cx: &mut App,
    build: impl FnOnce(&mut gpui::Context<T>) -> T,
) -> Box<dyn TabHandle> {
    Box::new(cx.new(build))
}
