use std::sync::Arc;

use gpui::{Action, App, Global, KeyBinding, SharedString};

use crate::types::{ClusterCaps, ResourceRef};

/// Decides whether an action applies to a resource on a cluster.
pub type Availability = Arc<dyn Fn(&ResourceRef, &ClusterCaps) -> bool>;

/// Metadata for one registered action.
#[derive(Clone)]
pub struct ActionSpec {
    /// Palette label, e.g. `Pod: Show Logs`.
    pub name: SharedString,
    /// Short label for the key-hint bar, e.g. `Logs`. `None` hides it from the bar.
    pub hint: Option<SharedString>,
    /// Default keystrokes in GPUI syntax, e.g. `l`, `ctrl-d`, `shift-f`.
    pub keystrokes: Option<SharedString>,
    /// Key context the binding is active in, e.g. `ResourceList`.
    pub context: Option<SharedString>,
    /// `None` = always available.
    pub available: Option<Availability>,
    action: Arc<dyn Action>,
}

impl ActionSpec {
    pub fn new(name: impl Into<SharedString>, action: impl Action) -> Self {
        Self {
            name: name.into(),
            hint: None,
            keystrokes: None,
            context: None,
            available: None,
            action: Arc::from(action.boxed_clone()),
        }
    }

    pub fn hint(mut self, hint: impl Into<SharedString>) -> Self {
        self.hint = Some(hint.into());
        self
    }

    /// Default key binding, active in `context` (a GPUI key context predicate).
    pub fn bind(mut self, keystrokes: impl Into<SharedString>, context: Option<&str>) -> Self {
        self.keystrokes = Some(keystrokes.into());
        self.context = context.map(|c| SharedString::from(c.to_string()));
        self
    }

    pub fn available_when(
        mut self,
        f: impl Fn(&ResourceRef, &ClusterCaps) -> bool + 'static,
    ) -> Self {
        self.available = Some(Arc::new(f));
        self
    }

    /// A fresh boxed copy of the action, ready to dispatch.
    pub fn action(&self) -> Box<dyn Action> {
        self.action.boxed_clone()
    }

    pub fn is_available(&self, target: &ResourceRef, caps: &ClusterCaps) -> bool {
        self.available.as_ref().is_none_or(|f| f(target, caps))
    }
}

/// All named actions. The palette lists them, the key-hint bar shows their hints, and list
/// views ask which ones apply to the selected resource.
///
/// ```ignore
/// ActionRegistry::register(
///     cx,
///     ActionSpec::new("Pod: Show Logs", ShowLogs)
///         .hint("Logs")
///         .bind("l", Some("ResourceList"))
///         .available_when(|r, _| r.gvr.resource == "pods"),
/// );
/// ```
#[derive(Default)]
pub struct ActionRegistry {
    specs: Vec<ActionSpec>,
}

impl Global for ActionRegistry {}

impl ActionRegistry {
    /// Adds an action and installs its default key binding.
    pub fn register(cx: &mut App, spec: ActionSpec) {
        if let Some(keys) = &spec.keystrokes {
            match KeyBinding::load(
                keys,
                spec.action(),
                spec.context.as_deref().map(parse_context),
                false,
                None,
                &gpui::DummyKeyboardMapper,
            ) {
                Ok(binding) => cx.bind_keys([binding]),
                Err(err) => tracing::error!(action = %spec.name, "invalid key binding: {err}"),
            }
        }
        cx.default_global::<Self>().specs.push(spec);
    }

    pub fn global(cx: &App) -> &Self {
        cx.global::<Self>()
    }

    pub fn all(&self) -> &[ActionSpec] {
        &self.specs
    }

    /// Replaces the keystrokes shown for the action at `index` in [`Self::all`] (key-hint bar,
    /// palette), after a keymap rebound it. Doesn't touch the key bindings themselves.
    pub fn set_keystrokes(cx: &mut App, index: usize, keystrokes: Option<SharedString>) {
        if let Some(spec) = cx.default_global::<Self>().specs.get_mut(index) {
            spec.keystrokes = keystrokes;
        }
    }

    /// Actions that apply to `target`.
    pub fn available_for<'a>(
        &'a self,
        target: &'a ResourceRef,
        caps: &'a ClusterCaps,
    ) -> impl Iterator<Item = &'a ActionSpec> + 'a {
        self.specs
            .iter()
            .filter(move |spec| spec.is_available(target, caps))
    }

    /// `(keystrokes, hint)` pairs for the key-hint bar of a key context, in registration order.
    pub fn hints(&self, context: &str) -> Vec<(SharedString, SharedString)> {
        self.specs
            .iter()
            .filter(|spec| spec.context.as_deref() == Some(context))
            .filter_map(|spec| Some((spec.keystrokes.clone()?, spec.hint.clone()?)))
            .collect()
    }
}

fn parse_context(context: &str) -> std::rc::Rc<gpui::KeyBindingContextPredicate> {
    gpui::KeyBindingContextPredicate::parse(context)
        .unwrap_or_else(|err| panic!("invalid key context {context:?}: {err}"))
        .into()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::types::{ClusterId, Gvr};

    gpui::actions!(test, [ShowLogs, Delete]);

    #[gpui::test]
    fn filters_by_availability_and_collects_hints(cx: &mut gpui::TestAppContext) {
        cx.update(|cx| {
            ActionRegistry::register(
                cx,
                ActionSpec::new("Pod: Show Logs", ShowLogs)
                    .hint("Logs")
                    .bind("l", Some("ResourceList"))
                    .available_when(|r, _| r.gvr.resource == "pods"),
            );
            ActionRegistry::register(
                cx,
                ActionSpec::new("Delete", Delete)
                    .hint("Delete")
                    .bind("ctrl-d", Some("ResourceList"))
                    .available_when(|_, caps| !caps.read_only),
            );

            let registry = ActionRegistry::global(cx);
            let deploy = ResourceRef::list(
                ClusterId::new("c"),
                Gvr::new("apps", "v1", "deployments"),
                None,
            );
            let read_only = ClusterCaps {
                read_only: true,
                ..Default::default()
            };
            let names: Vec<_> = registry
                .available_for(&deploy, &read_only)
                .map(|s| s.name.to_string())
                .collect();
            assert!(names.is_empty());
            let names: Vec<_> = registry
                .available_for(&deploy, &ClusterCaps::default())
                .map(|s| s.name.to_string())
                .collect();
            assert_eq!(names, ["Delete"]);

            let hints = registry.hints("ResourceList");
            assert_eq!(hints.len(), 2);
            assert_eq!(hints[0].1.as_ref(), "Logs");
            assert!(registry.all()[0].action().partial_eq(&ShowLogs));

            ActionRegistry::set_keystrokes(cx, 0, Some("shift-l".into()));
            let hints = ActionRegistry::global(cx).hints("ResourceList");
            assert_eq!(hints[0].0.as_ref(), "shift-l");
        });
    }
}
